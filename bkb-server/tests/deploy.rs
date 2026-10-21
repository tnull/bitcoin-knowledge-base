//! Exercise deployment with local command doubles; never contact a VPS.
#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

struct Deployment {
	root: PathBuf,
}

impl Deployment {
	fn new() -> Self {
		let root = std::env::temp_dir().join(format!(
			"bkb-deploy-test-{}-{}",
			std::process::id(),
			NEXT_ID.fetch_add(1, Ordering::Relaxed)
		));
		fs::create_dir(&root).unwrap();
		for dir in ["contrib", "bin", "target/release"] {
			fs::create_dir_all(root.join(dir)).unwrap();
		}
		let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../contrib");
		for file in ["deploy.sh", "bkb-server.service"] {
			fs::copy(source.join(file), root.join("contrib").join(file)).unwrap();
		}
		fs::write(root.join("target/release/bkb-server"), "stale").unwrap();
		let deployment = Self { root };
		deployment.stub("git", "printf '%s\\n' master");
		deployment.stub(
			"cargo",
			r#"printf '%s' "${CARGO_TARGET_DIR:-target}" > "$DEPLOY_TEST_ROOT/build-target"
printf '%s\n' "$@" > "$DEPLOY_TEST_ROOT/build-args"
if [[ ${MOCK_BUILD_FAIL:-0} == 1 ]]; then exit 1; fi
if [[ ${MOCK_SKIP_BINARY:-0} != 1 ]]; then
    mkdir -p "${CARGO_TARGET_DIR:-target}/release"
    printf fresh > "${CARGO_TARGET_DIR:-target}/release/bkb-server"
fi"#,
		);
		deployment.stub(
			"scp",
			r#"printf '%s\n' "$@" >> "$DEPLOY_TEST_ROOT/scp-args"
case "$1" in
    */bkb-server)
        cp "$1" "$DEPLOY_TEST_ROOT/uploaded-binary"
        cp "$1" "$DEPLOY_TEST_ROOT/bkb-server.new"
        ;;
esac"#,
		);
		deployment.stub(
			"ssh",
			r#"printf '%s\n' "$@" >> "$DEPLOY_TEST_ROOT/ssh-args"
if [[ ${2:-} == *'/etc/bkb-server.env'* ]]; then
    if [[ ${MOCK_ENV_FAIL:-0} == 1 ]]; then exit 1; fi
    # Only the environment installation runs, in our disposable directory.
    command=${2//\/etc\/bkb-server.env/$DEPLOY_TEST_ROOT\/remote.env}
    bash -c "$command"
else
    script=$(cat)
    printf '%s\n' "$script" >> "$DEPLOY_TEST_ROOT/ssh-script"
    if [[ $script == *'systemctl restart'* ]]; then
        script=${script//\/opt\/bkb/$DEPLOY_TEST_ROOT}
        shift
        "$@" <<< "$script"
    fi
fi"#,
		);
		deployment.stub(
			"systemctl",
			r#"printf '%s\n' "$*" >> "$DEPLOY_TEST_ROOT/systemctl-calls"
if [[ $1 == is-active && ${MOCK_HEALTH:-ready} == crashed ]]; then exit 3; fi"#,
		);
		deployment.stub("journalctl", "printf 'startup diagnostics\\n'");
		deployment.stub("sleep", "exit 0");
		deployment.stub(
			"curl",
			r#"if [[ $* == *https://* ]]; then
    if [[ ${MOCK_PUBLIC_FAILURE:-0} == 1 ]]; then exit 22; fi
    exit 0
fi
count=0
if [[ -f "$DEPLOY_TEST_ROOT/health-attempts" ]]; then
    count=$(cat "$DEPLOY_TEST_ROOT/health-attempts")
fi
printf '%s' "$((count + 1))" > "$DEPLOY_TEST_ROOT/health-attempts"
case ${MOCK_HEALTH:-ready} in
    crashed|timeout) exit 7 ;;
    delayed) if (( count < 2 )); then exit 7; fi ;;
esac"#,
		);
		deployment
	}

	fn stub(&self, name: &str, script: &str) {
		let path = self.root.join("bin").join(name);
		fs::write(&path, format!("#!/bin/bash\nset -euo pipefail\n{script}\n")).unwrap();
		fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
	}

	fn run(&self, env: &[(&str, &str)], input: &str) -> Output {
		let mut command = Command::new("bash");
		command
			.arg(self.root.join("contrib/deploy.sh"))
			.current_dir(&self.root)
			.env_clear()
			.env("PATH", format!("{}:/usr/bin:/bin", self.root.join("bin").display()))
			.env("DEPLOY_TEST_ROOT", &self.root)
			.env("CARGO_TARGET_DIR", self.root.join("custom target"))
			.envs(env.iter().copied())
			.stdin(Stdio::piped())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped());
		let mut child = command.spawn().unwrap();
		child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
		child.wait_with_output().unwrap()
	}

	fn read(&self, file: &str) -> String {
		fs::read_to_string(self.root.join(file)).unwrap_or_default()
	}
}

impl Drop for Deployment {
	fn drop(&mut self) {
		fs::remove_dir_all(&self.root).unwrap();
	}
}

const CREDENTIALS: &[(&str, &str)] = &[
	("GITHUB_TOKEN", "github-test-token"),
	("FORGEJO_TOKEN", "forgejo-test-token"),
	("BKB_ADMIN_PASSWORD", "admin-test-password"),
];

fn assert_success(output: &Output) {
	assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[test]
fn uploads_the_binary_from_the_configured_target() {
	let deployment = Deployment::new();
	assert_success(&deployment.run(CREDENTIALS, ""));
	assert_eq!(deployment.read("uploaded-binary"), "fresh", "must not upload a stale binary");
	assert!(deployment.read("build-args").lines().any(|arg| arg == "--locked"));
}

#[test]
fn installs_all_credentials_privately_without_shell_expansion() {
	let deployment = Deployment::new();
	let password = " spaces ' \" \\ $dollar `backtick` %percent ";
	let output =
		deployment.run(&[CREDENTIALS[0], CREDENTIALS[1], ("BKB_ADMIN_PASSWORD", password)], "");
	assert_success(&output);
	let environment = deployment.read("remote.env");
	assert!(environment.contains("FORGEJO_TOKEN=\"forgejo-test-token\"\n"));
	assert!(environment.contains("GITHUB_TOKEN=\"github-test-token\"\n"));
	assert!(environment
		.contains(r#"BKB_ADMIN_PASSWORD=" spaces ' \" \\ $dollar `backtick` %percent ""#));
	assert_eq!(
		fs::metadata(deployment.root.join("remote.env")).unwrap().permissions().mode() & 0o777,
		0o600
	);
	assert!(deployment
		.read("contrib/bkb-server.service")
		.contains("EnvironmentFile=/etc/bkb-server.env"));
	let arguments = deployment.read("ssh-args");
	let output = format!(
		"{}{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	for value in ["github-test-token", "forgejo-test-token", password] {
		assert!(!arguments.contains(value), "credentials must not be SSH arguments");
		assert!(!output.contains(value), "credentials must not be printed");
	}
}

#[test]
fn prompts_for_credentials_missing_from_environment() {
	let deployment = Deployment::new();
	let output = deployment.run(&[], "github-prompted\nforgejo-prompted\nadmin-prompted\n");
	assert_success(&output);
	for (name, value) in [
		("GITHUB_TOKEN", "github-prompted"),
		("FORGEJO_TOKEN", "forgejo-prompted"),
		("BKB_ADMIN_PASSWORD", "admin-prompted"),
	] {
		assert!(deployment.read("remote.env").contains(&format!("{name}=\"{value}\"")));
		assert!(!String::from_utf8_lossy(&output.stdout).contains(value));
		assert!(!String::from_utf8_lossy(&output.stderr).contains(value));
	}
}

#[test]
fn explicit_empty_admin_password_disables_admin_endpoints() {
	let deployment = Deployment::new();
	assert_success(
		&deployment.run(&[CREDENTIALS[0], CREDENTIALS[1], ("BKB_ADMIN_PASSWORD", "")], ""),
	);
	assert!(deployment.root.join("remote.env").exists(), "must install service environment");
	assert!(!deployment.read("remote.env").contains("BKB_ADMIN_PASSWORD="));
	assert!(!deployment
		.read("contrib/bkb-server.service")
		.contains("Environment=BKB_ADMIN_PASSWORD="));
}

#[test]
fn missing_or_empty_token_stops_before_building() {
	for token in [None, Some("")] {
		let deployment = Deployment::new();
		let mut env = vec![CREDENTIALS[0], CREDENTIALS[2]];
		if let Some(token) = token {
			env.push(("FORGEJO_TOKEN", token));
		}
		let output = deployment.run(&env, "");
		assert!(!output.status.success(), "missing Forgejo credentials must stop deployment");
		assert!(String::from_utf8_lossy(&output.stderr).contains("FORGEJO_TOKEN"));
		assert!(!deployment.root.join("build-args").exists());
		assert!(!deployment.root.join("ssh-args").exists());
	}
}

#[test]
fn failed_or_missing_build_never_uploads_stale_binary() {
	for failure in ["MOCK_BUILD_FAIL", "MOCK_SKIP_BINARY"] {
		let deployment = Deployment::new();
		let mut env = CREDENTIALS.to_vec();
		env.push((failure, "1"));
		assert!(
			!deployment.run(&env, "").status.success(),
			"must require a successful build artifact"
		);
		assert!(!deployment.root.join("ssh-args").exists());
		assert!(!deployment.root.join("scp-args").exists());
	}
}

#[test]
fn failed_credential_installation_never_restarts_service() {
	let deployment = Deployment::new();
	let mut env = CREDENTIALS.to_vec();
	env.push(("MOCK_ENV_FAIL", "1"));
	assert!(
		!deployment.run(&env, "").status.success(),
		"must stop if credentials cannot be installed"
	);
	assert!(!deployment.read("ssh-script").contains("systemctl restart"));
}

#[test]
fn waits_for_http_readiness_before_reporting_success() {
	let deployment = Deployment::new();
	let mut env = CREDENTIALS.to_vec();
	env.push(("MOCK_HEALTH", "delayed"));
	assert_success(&deployment.run(&env, ""));
	assert_eq!(deployment.read("health-attempts"), "3", "must wait for the API to respond");
}

#[test]
fn startup_failure_or_timeout_cannot_report_success() {
	for mode in ["crashed", "timeout"] {
		let deployment = Deployment::new();
		let mut env = CREDENTIALS.to_vec();
		env.extend([("MOCK_HEALTH", mode), ("DEPLOY_HEALTH_TIMEOUT", "1")]);
		let output = deployment.run(&env, "");
		assert!(!output.status.success(), "an unavailable API must fail deployment: {mode}");
		assert!(String::from_utf8_lossy(&output.stdout).contains("startup diagnostics"));
		assert!(!String::from_utf8_lossy(&output.stdout).contains("deployed and running"));
	}
}

#[test]
fn public_proxy_failure_cannot_report_success() {
	let deployment = Deployment::new();
	let mut env = CREDENTIALS.to_vec();
	env.push(("MOCK_PUBLIC_FAILURE", "1"));
	let output = deployment.run(&env, "");
	assert!(!output.status.success(), "a public HTTP failure must fail deployment");
	assert!(String::from_utf8_lossy(&output.stderr).contains("public health check failed"));
}
