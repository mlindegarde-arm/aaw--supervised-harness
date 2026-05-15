use crate::error::{HarnessError, HarnessResult};
use clap_complete::{Shell as ClapShell, generate};
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "bash" => Ok(Self::Bash),
            "zsh" => Ok(Self::Zsh),
            "fish" => Ok(Self::Fish),
            other => Err(format!(
                "unsupported shell {other:?}; expected bash, zsh, or fish"
            )),
        }
    }

    fn clap_shell(self) -> ClapShell {
        match self {
            Self::Bash => ClapShell::Bash,
            Self::Zsh => ClapShell::Zsh,
            Self::Fish => ClapShell::Fish,
        }
    }
}

pub fn write_completion(shell: Shell, writer: &mut dyn Write) -> HarnessResult<()> {
    let mut command = crate::runtime::build_clap();
    generate(shell.clap_shell(), &mut command, "harness", writer);
    Ok(())
}

pub fn completion_script(shell: Shell) -> HarnessResult<String> {
    let mut output = Vec::new();
    write_completion(shell, &mut output)?;
    String::from_utf8(output).map_err(|err| {
        HarnessError::External(format!("generated completion output was not UTF-8: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use crate::cli;

    #[test]
    fn bash_completion_command_outputs_script_smoke() {
        let output = run_completion_command("bash");

        assert!(output.contains("_harness()"), "{output}");
        assert!(output.contains("task"), "{output}");
        assert!(output.contains("objective"), "{output}");
        assert!(output.contains("completions"), "{output}");
        assert!(output.contains("planning"), "{output}");
        assert!(output.contains("ready"), "{output}");
        assert!(output.contains("resolved"), "{output}");
    }

    #[test]
    fn zsh_completion_command_outputs_script_smoke() {
        let output = run_completion_command("zsh");

        assert!(output.contains("#compdef harness"), "{output}");
        assert!(output.contains("_harness"), "{output}");
        assert!(output.contains("supervise"), "{output}");
        assert!(output.contains("objective"), "{output}");
        assert!(output.contains("planning"), "{output}");
        assert!(output.contains("ready"), "{output}");
        assert!(output.contains("resolved"), "{output}");
    }

    #[test]
    fn fish_completion_command_outputs_script_smoke() {
        let output = run_completion_command("fish");

        assert!(output.contains("complete -c harness"), "{output}");
        assert!(output.contains("task"), "{output}");
        assert!(output.contains("objective"), "{output}");
        assert!(output.contains("completions"), "{output}");
        assert!(output.contains("planning"), "{output}");
        assert!(output.contains("ready"), "{output}");
        assert!(output.contains("resolved"), "{output}");
    }

    #[test]
    fn unsupported_shell_returns_usage_error() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = cli::run_with_io(
            ["harness", "completions", "powershell"],
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(exit.code(), 2);
        assert!(stdout.is_empty());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("unsupported shell"), "{stderr}");
        assert!(stderr.contains("bash, zsh, or fish"), "{stderr}");
    }

    fn run_completion_command(shell: &str) -> String {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let exit = cli::run_with_io(["harness", "completions", shell], &mut stdout, &mut stderr);

        assert_eq!(exit.code(), 0);
        assert!(stderr.is_empty(), "{}", String::from_utf8_lossy(&stderr));
        String::from_utf8(stdout).unwrap()
    }
}
