use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

struct StopSpawning<'a>(&'a AtomicBool);

impl Drop for StopSpawning<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[test]
fn executables_run_immediately_while_other_threads_spawn() {
    let directory = TemporaryDirectory::for_test("executable 'concurrent fork");
    let path = directory.path("fixture");
    let stop = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let _stop = StopSpawning(&stop);
        for _ in 0..2 {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    assert!(
                        std::process::Command::new("/bin/sh")
                            .args(["-c", ":"])
                            .status()
                            .expect("spawn concurrent process")
                            .success()
                    );
                }
            });
        }
        for value in 0..32 {
            write_executable(&path, format!("#!/bin/sh\nprintf '%s' '{value}'\n"), 0o700)
                .expect("publish executable fixture");
            let output = std::process::Command::new(&path)
                .output()
                .expect("execute freshly written fixture");
            assert!(output.status.success(), "{output:?}");
            assert_eq!(output.stdout, value.to_string().as_bytes());
        }
    });
}
