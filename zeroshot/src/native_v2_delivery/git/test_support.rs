use std::collections::BTreeMap;
use std::path::PathBuf;

use openengine_cluster_testkit::assertions::AssertValue;

use super::SystemGit;

pub(crate) enum ConfigLocation {
    Home,
    Xdg,
    Explicit,
}

pub(crate) struct GitFixture {
    pub(crate) root: tempfile::TempDir,
    pub(crate) workspace: PathBuf,
    pub(crate) program: PathBuf,
    pub(crate) environment: BTreeMap<String, String>,
}

impl GitFixture {
    pub(crate) fn new(config: &str, location: ConfigLocation) -> Self {
        let root = tempfile::tempdir().assert_value();
        let workspace = root.path().join("workspace");
        let home = root.path().join("home");
        let xdg = root.path().join("xdg");
        for directory in [&workspace, &home, &xdg.join("git")] {
            std::fs::create_dir_all(directory).assert_value();
        }
        let mut environment = BTreeMap::new();
        crate::execution::platform::process_environment(&mut environment);
        environment.extend([
            ("PATH".to_owned(), std::env::var("PATH").assert_value()),
            ("HOME".to_owned(), home.to_str().assert_value().to_owned()),
            (
                "USERPROFILE".to_owned(),
                home.to_str().assert_value().to_owned(),
            ),
            (
                "XDG_CONFIG_HOME".to_owned(),
                xdg.to_str().assert_value().to_owned(),
            ),
            ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
        ]);
        let configuration = match location {
            ConfigLocation::Home => home.join(".gitconfig"),
            ConfigLocation::Xdg => xdg.join("git/config"),
            ConfigLocation::Explicit => {
                let path = root.path().join("explicit.gitconfig");
                environment.insert(
                    "GIT_CONFIG_GLOBAL".to_owned(),
                    path.to_str().assert_value().to_owned(),
                );
                // This conflicting home setting must lose to the explicit path.
                std::fs::write(home.join(".gitconfig"), "[core]\n autocrlf = false\n")
                    .assert_value();
                path
            }
        };
        std::fs::write(configuration, config).assert_value();
        let program = crate::execution::platform::find_executable("git", &environment, &workspace)
            .assert_value();
        let fixture = Self {
            root,
            workspace,
            program,
            environment,
        };
        fixture.git(&["init", "--initial-branch=main"]);
        fixture
    }

    /// Ordinary Git in an isolated user environment, independent of delivery's command builder.
    pub(crate) fn git(&self, arguments: &[&str]) -> Vec<u8> {
        let output = std::process::Command::new(&self.program)
            .env_clear()
            .envs(&self.environment)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
            ])
            .arg("-C")
            .arg(&self.workspace)
            .args(arguments)
            .output()
            .assert_value();
        assert!(
            output.status.success(),
            "Git {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }

    pub(crate) fn seed(&self) -> String {
        std::fs::write(self.workspace.join("sample.txt"), b"first\r\nsecond\r\n").assert_value();
        self.git(&["add", "--all"]);
        self.git(&["commit", "-m", "base"]);
        String::from_utf8(self.git(&["rev-parse", "HEAD"]))
            .assert_value()
            .trim()
            .to_owned()
    }

    pub(crate) fn edit(&self) {
        std::fs::write(self.workspace.join("sample.txt"), b"first\r\nchanged\r\n").assert_value();
    }

    pub(crate) fn system_git(&self) -> SystemGit {
        SystemGit::new(self.program.clone()).with_local_environment(Some(self.environment.clone()))
    }

    pub(crate) async fn assert_delivered(&self, system: &SystemGit, base: &str, bytes: &[u8]) {
        let revision = system
            .prepare_revision(&self.workspace, base, "fix: one line")
            .await
            .assert_value();
        self.assert_committed(system, &revision, bytes).await;
        assert_eq!(
            self.git(&["diff", "--numstat", base, &revision, "--", "sample.txt"]),
            b"1\t1\tsample.txt\n"
        );
    }

    pub(crate) async fn assert_committed(&self, system: &SystemGit, revision: &str, bytes: &[u8]) {
        assert_eq!(self.git(&["show", "HEAD:sample.txt"]), bytes);
        assert_eq!(
            system.workspace_state(&self.workspace).await.assert_value(),
            (revision.to_owned(), false)
        );
    }
}
