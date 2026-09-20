use std::env;

use clap::ValueEnum;

/// shell 統合が読み込まれていることを、ラッパー関数が子プロセスへ伝えるための環境変数。
pub const SHELL_INTEGRATION_ENV: &str = "ATCLI_SHELL_INTEGRATION";

const ZSH: &str = include_str!("shell/init.zsh");

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Shell {
    Zsh,
}

pub fn run(shell: Shell) {
    match shell {
        Shell::Zsh => print!("{ZSH}"),
    }
}

/// `--cd` や `atcli cd` を shell 統合なしで呼んだときに、一度だけ導入方法を案内する。
///
/// パス自体は stdout に出るので、`cd "$(atcli cd abc300 b)"` のような使い方は案内後も成立する。
pub fn warn_unless_integrated(command: &str) {
    if env::var_os(SHELL_INTEGRATION_ENV).is_none() {
        eprintln!(
            "{command} で移動するには shell 統合が必要です: eval \"$(atcli init zsh)\" を ~/.zshrc に追加してください"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_integration_defines_both_entry_points() {
        assert!(ZSH.contains("atcli()"), "{ZSH}");
        assert!(ZSH.contains("atcd()"), "{ZSH}");
        // ラッパーが自分自身を呼び出すと無限再帰するため、必ず `command atcli` を通す。
        assert!(!ZSH.contains("\n  atcli "), "{ZSH}");
    }

    #[test]
    fn zsh_integration_marks_the_environment() {
        assert!(ZSH.contains(SHELL_INTEGRATION_ENV), "{ZSH}");
    }
}
