use std::process::Command;

pub(crate) fn command() -> Command {
    let command = Command::new("git");
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let mut command = command;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        command
    };
    command
}
