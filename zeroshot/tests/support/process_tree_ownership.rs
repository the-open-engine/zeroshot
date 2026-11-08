//! Real escaped descendants shared by the process runner and local-controller regressions.
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use openengine_cluster_testkit::TemporaryDirectory;
use openengine_cluster_testkit::assertions::AssertValue;
use tokio::time::{Instant, sleep};

const WRITER: &str = r#"import os,sys,time
mode,root=sys.argv[1:]
if mode=='setpgid': os.setpgid(0,0)
else: os.setsid()
if mode=='double-fork' and os.fork(): os._exit(0)
with open(root+'/tool.pid','w') as f: f.write(str(os.getpid()))
while True:
 with open(root+'/tool.log','a') as f: f.write('tick\n')
 time.sleep(.025)
"#;

pub struct EscapedTool {
    root: TemporaryDirectory,
    pub script: PathBuf,
    pub python: String,
}
impl EscapedTool {
    pub fn new() -> Self {
        let root = TemporaryDirectory::for_test("zs-owned-tree");
        let script = root.path("writer.py");
        fs::write(&script, WRITER).assert_value();
        let output = Command::new("python3")
            .args(["-c", "import sys;print(sys.executable)"])
            .output()
            .assert_value();
        assert!(output.status.success(), "Python prerequisite failed");
        Self {
            root,
            script,
            python: String::from_utf8(output.stdout)
                .assert_value()
                .trim()
                .to_owned(),
        }
    }
    pub fn directory(&self) -> PathBuf {
        self.root.path("")
    }
    pub async fn wait_started(&self) -> i32 {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(text) = fs::read_to_string(self.root.path("tool.pid")) {
                if let Ok(pid) = text.trim().parse() {
                    return pid;
                }
            }
            assert!(Instant::now() < end, "escaped writer did not start");
            sleep(Duration::from_millis(10)).await;
        }
    }
    pub fn live(&self) -> bool {
        let Ok(text) = fs::read_to_string(self.root.path("tool.pid")) else {
            return false;
        };
        let Ok(pid) = text.trim().parse::<i32>() else {
            return false;
        };
        let own = fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|argv| {
            argv.split(|byte| *byte == 0)
                .any(|arg| arg == self.script.as_os_str().as_encoded_bytes())
        });
        own && live(pid)
    }
}
impl Drop for EscapedTool {
    fn drop(&mut self) {
        if let Ok(text) = fs::read_to_string(self.root.path("tool.pid")) {
            if let Ok(pid) = text.trim().parse::<i32>() {
                if self.live() {
                    // SAFETY: this PID is the live unique fixture writer, not a reused process.
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                    }
                }
            }
        }
    }
}
pub fn live(pid: i32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    stat.rfind(") ")
        .and_then(|i| stat.as_bytes().get(i + 2))
        .is_some_and(|s| !matches!(*s, b'Z' | b'X'))
}
