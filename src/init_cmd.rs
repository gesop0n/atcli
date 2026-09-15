use clap::ValueEnum;

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
}
