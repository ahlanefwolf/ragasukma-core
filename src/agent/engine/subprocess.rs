use std::process::Command;

/// Utility helper to configure subprocess to be isolated from AppImage runtime and prevent console flash on Windows
#[allow(unused_variables)]
pub fn configure_subprocess(cmd: &mut Command) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW (0x08000000)
        cmd.creation_flags(0x08000000);
    }
    #[cfg(target_os = "linux")]
    {
        cmd.env_remove("PYTHONHOME");
        cmd.env_remove("PYTHONPATH");
        cmd.env_remove("LD_LIBRARY_PATH");
    }
}
