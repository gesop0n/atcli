use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

/// How to invoke the C++ compiler.
#[derive(Clone, Debug)]
pub struct BuildSettings {
    pub compiler: String,
    pub standard: String,
    /// Absolute include directories passed as `-I`.
    pub include_dirs: Vec<PathBuf>,
    pub flags: Vec<String>,
}

/// A single compilation, cached under `build_dir`.
#[derive(Clone, Copy, Debug)]
pub struct BuildRequest<'a> {
    pub source: &'a Path,
    pub build_dir: &'a Path,
    /// File name of the produced binary, which also keys the cache.
    pub binary_name: &'a str,
    pub settings: &'a BuildSettings,
    pub rebuild: bool,
}

#[derive(Clone, Debug)]
pub struct BuildOutcome {
    pub binary: PathBuf,
    /// Whether a previous build was reused instead of running the compiler.
    pub cached: bool,
}

/// What the compiler said when it rejected the solution.
#[derive(Clone, Debug)]
pub struct CompileError {
    pub stdout: String,
    pub stderr: String,
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("C++ のコンパイルに失敗しました")
    }
}

impl Error for CompileError {}

/// Compile `request.source`, reusing the cached binary when neither the sources
/// it depends on nor the compiler invocation have changed.
///
/// # Errors
///
/// Returns a [`CompileError`] when the compiler rejects the solution, and other
/// errors when the build directory or compiler cannot be used.
pub fn build(request: &BuildRequest<'_>) -> Result<BuildOutcome> {
    if !request.source.is_file() {
        bail!("解答ファイルが見つかりません: {}", request.source.display());
    }
    fs::create_dir_all(request.build_dir).with_context(|| {
        format!(
            "ビルドディレクトリを作成できません: {}",
            request.build_dir.display()
        )
    })?;

    let binary = request.build_dir.join(request.binary_name);
    let depfile = binary.with_extension("d");
    let fingerprint_file = binary.with_extension("fingerprint");
    let signature = compiler_signature(request)?;

    if !request.rebuild && cache_is_fresh(&binary, &depfile, &fingerprint_file, &signature) {
        return Ok(BuildOutcome {
            binary,
            cached: true,
        });
    }

    let mut command = Command::new(&request.settings.compiler);
    command
        .arg(request.source)
        .arg(format!("-std={}", request.settings.standard))
        .args(&request.settings.flags);
    for include_dir in &request.settings.include_dirs {
        command.arg(format!("-I{}", include_dir.display()));
    }
    command.arg("-MMD").arg("-MF").arg(&depfile);
    command.arg("-o").arg(&binary);

    let output = command.output().with_context(|| {
        format!(
            "C++ コンパイラを実行できません: {}",
            request.settings.compiler
        )
    })?;
    if !output.status.success() {
        return Err(CompileError {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
        .into());
    }

    if let Ok(dependencies) = read_dependencies(&depfile) {
        let fingerprint = fingerprint(&signature, &dependencies)?;
        fs::write(&fingerprint_file, format!("{fingerprint:016x}\n")).with_context(|| {
            format!(
                "ビルドキャッシュ情報を書き込めません: {}",
                fingerprint_file.display()
            )
        })?;
    }
    Ok(BuildOutcome {
        binary,
        cached: false,
    })
}

fn compiler_signature(request: &BuildRequest<'_>) -> Result<String> {
    let settings = request.settings;
    let version = Command::new(&settings.compiler)
        .arg("--version")
        .output()
        .with_context(|| format!("C++ コンパイラを実行できません: {}", settings.compiler))?;
    Ok(format!(
        "compiler={}\nversion={}\nsource={}\nstandard={}\nflags={:?}\nincludes={:?}\n",
        settings.compiler,
        String::from_utf8_lossy(&version.stdout),
        request.source.display(),
        settings.standard,
        settings.flags,
        settings.include_dirs,
    ))
}

fn cache_is_fresh(
    binary: &Path,
    depfile: &Path,
    fingerprint_file: &Path,
    command_signature: &str,
) -> bool {
    if !binary.is_file() || !fingerprint_file.is_file() {
        return false;
    }
    let Ok(dependencies) = read_dependencies(depfile) else {
        return false;
    };
    let Ok(current) = fingerprint(command_signature, &dependencies) else {
        return false;
    };
    fs::read_to_string(fingerprint_file)
        .is_ok_and(|stored| stored.trim() == format!("{current:016x}"))
}

fn read_dependencies(depfile: &Path) -> Result<Vec<PathBuf>> {
    let contents = fs::read_to_string(depfile)
        .with_context(|| format!("依存ファイルを読めません: {}", depfile.display()))?;
    parse_dependencies(&contents)
}

fn parse_dependencies(contents: &str) -> Result<Vec<PathBuf>> {
    let contents = contents.replace("\\\r\n", " ").replace("\\\n", " ");
    let (_, dependencies) = contents
        .split_once(':')
        .context("依存ファイルの形式が不正です")?;
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in dependencies.chars() {
        if escaped {
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character.is_whitespace() {
            if !current.is_empty() {
                paths.push(PathBuf::from(std::mem::take(&mut current)));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        paths.push(PathBuf::from(current));
    }
    if paths.is_empty() {
        bail!("依存ファイルに入力がありません");
    }
    Ok(paths)
}

fn fingerprint(command_signature: &str, dependencies: &[PathBuf]) -> Result<u64> {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_bytes(&mut hash, command_signature.as_bytes());
    for dependency in dependencies {
        hash_bytes(&mut hash, dependency.as_os_str().as_encoded_bytes());
        let contents = fs::read(dependency)
            .with_context(|| format!("依存ファイルを読めません: {}", dependency.display()))?;
        hash_bytes(&mut hash, &contents);
    }
    Ok(hash)
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use super::{cache_is_fresh, fingerprint, parse_dependencies};

    #[test]
    fn parses_makefile_dependencies_with_continuations_and_spaces() {
        let depfile = concat!("main: /tmp/main.cpp \\", "\n", " /tmp/local\\ header.hpp\n");
        let dependencies = parse_dependencies(depfile).unwrap();
        assert_eq!(
            dependencies,
            [
                PathBuf::from("/tmp/main.cpp"),
                PathBuf::from("/tmp/local header.hpp"),
            ]
        );
    }

    #[test]
    fn invalidates_cache_when_dependency_or_command_changes() {
        let temp = tempdir().unwrap();
        let source = temp.path().join("main.cpp");
        let header = temp.path().join("local.hpp");
        let binary = temp.path().join("main");
        let depfile = temp.path().join("main.d");
        let fingerprint_file = temp.path().join("main.fingerprint");
        fs::write(&source, "#include \"local.hpp\"\n").unwrap();
        fs::write(&header, "constexpr int answer = 42;\n").unwrap();
        fs::write(&binary, "binary").unwrap();
        fs::write(
            &depfile,
            format!("main: {} {}\n", source.display(), header.display()),
        )
        .unwrap();
        let dependencies = [source, header.clone()];
        let current = fingerprint("g++ -O0", &dependencies).unwrap();
        fs::write(&fingerprint_file, format!("{current:016x}\n")).unwrap();

        assert!(cache_is_fresh(
            &binary,
            &depfile,
            &fingerprint_file,
            "g++ -O0"
        ));
        assert!(!cache_is_fresh(
            &binary,
            &depfile,
            &fingerprint_file,
            "g++ -O2"
        ));

        fs::write(header, "constexpr int answer = 43;\n").unwrap();
        assert!(!cache_is_fresh(
            &binary,
            &depfile,
            &fingerprint_file,
            "g++ -O0"
        ));
    }
}
