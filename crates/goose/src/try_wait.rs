use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use process_wrap::std::{CommandWrap, ProcessSession};

pub fn run_try_wait() {
    let mut cmd = CommandWrap::from(Command::new("bash"));
    cmd.command_mut().args(["-l", "-i", "-c", "echo $PATH"]);
    cmd.command_mut().stdin(Stdio::null());
    cmd.command_mut().stdout(Stdio::piped());
    cmd.command_mut().stderr(Stdio::null());
    cmd.wrap(ProcessSession);
    let mut child = cmd.spawn().unwrap();
    
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            println!("Exited with {}", status);
            break;
        }
        if Instant::now() > deadline {
            println!("Timed out, killing");
            child.kill().unwrap();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
