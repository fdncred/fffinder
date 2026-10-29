use fffinder::args;

fn main() {
    match args::parse(std::env::args_os().skip(1)) {
        Ok(args::Parsed::Run(args)) if args.detach && !args.pick => {
            // The copy started below has no terminal to tell it where the
            // shell is, so the folder goes with it.
            detach(
                args.path
                    .is_none()
                    .then(|| fffinder::shell_folder(false))
                    .flatten(),
            )
        }
        Ok(args::Parsed::Run(args)) => {
            // Before GPUI, or any other thread, starts.
            fffinder::graphics::prepare(args.software_rendering);
            fffinder::run(args)
        }
        Ok(args::Parsed::Help) => print!("{}", args::USAGE),
        Ok(args::Parsed::Version) => println!("fffinder {}", env!("CARGO_PKG_VERSION")),
        Err(err) => {
            eprintln!("fffinder: {err}\n\n{}", args::USAGE);
            std::process::exit(2);
        }
    }
}

/// Run again without `--detach`, in a process group of its own with no
/// terminal attached, so the window outlives the shell that started it.
/// `folder` is added to the arguments as the folder to open.
fn detach(folder: Option<std::path::PathBuf>) {
    let me = std::env::current_exe().unwrap_or_else(|_| "fffinder".into());
    let args = std::env::args_os().skip(1).filter(|a| a != "--detach");
    let mut command = std::process::Command::new(me);
    command
        .args(args)
        .args(folder)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    if let Err(err) = command.spawn() {
        eprintln!("fffinder: could not start: {err}");
        std::process::exit(1);
    }
}
