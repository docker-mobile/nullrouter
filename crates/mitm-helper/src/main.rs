//! Command-line entrypoint for the fixed-command MITM helper.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use nullrouter_mitm_helper::{
    HelperError, Tool, ca, disable_hosts, elevated, enable_hosts, hosts_enabled,
};

const HOSTS: &str = "/etc/hosts";
const LINUX_TRUST_DIR: &str = "/usr/local/share/ca-certificates";
const LINUX_TRUST_NAME: &str = "nullrouter-mitm-root-ca.crt";
const UPDATE_CA_CERTIFICATES: &str = "/usr/sbin/update-ca-certificates";

#[allow(
    clippy::print_stdout,
    reason = "the fixed-command helper reports one machine-readable result"
)]
fn main() -> std::process::ExitCode {
    match run(env::args().skip(1)) {
        Ok(text) => {
            println!("{text}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<&'static str, HelperError> {
    if !elevated() {
        return Err(HelperError::NotElevated);
    }
    let operation = args
        .next()
        .ok_or_else(|| HelperError::UnknownTool("missing operation".to_owned()))?;
    let parameter = args.next();
    if args.next().is_some() {
        return Err(HelperError::UnknownTool("unexpected argument".to_owned()));
    }
    match operation.as_str() {
        "enable-hosts" => {
            enable_hosts(Path::new(HOSTS), required_tool(parameter)?)?;
            Ok("hosts enabled")
        }
        "disable-hosts" => {
            disable_hosts(Path::new(HOSTS), required_tool(parameter)?)?;
            Ok("hosts disabled")
        }
        "hosts-status" => {
            if hosts_enabled(Path::new(HOSTS), required_tool(parameter)?)? {
                Ok("enabled")
            } else {
                Ok("disabled")
            }
        }
        "ensure-ca" => {
            no_parameter(parameter.as_ref())?;
            let _authority = ca::ensure(&mitm_dir()?)?;
            Ok("ca ready")
        }
        "install-ca" => {
            no_parameter(parameter.as_ref())?;
            install_ca()?;
            Ok("CA trusted")
        }
        "uninstall-ca" => {
            no_parameter(parameter.as_ref())?;
            uninstall_ca()?;
            Ok("CA untrusted")
        }
        _other => Err(HelperError::UnknownTool(operation)),
    }
}

fn required_tool(parameter: Option<String>) -> Result<Tool, HelperError> {
    Tool::parse(&parameter.ok_or_else(|| HelperError::UnknownTool("missing tool".to_owned()))?)
}

fn no_parameter(parameter: Option<&String>) -> Result<(), HelperError> {
    if parameter.is_some() {
        Err(HelperError::UnknownTool("unexpected argument".to_owned()))
    } else {
        Ok(())
    }
}

fn mitm_dir() -> Result<PathBuf, HelperError> {
    let data = env::var_os("DATA_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".nullrouter"))
        })
        .ok_or_else(|| HelperError::UnknownTool("DATA_DIR or HOME is required".to_owned()))?;
    Ok(data.join("mitm"))
}

#[cfg(target_os = "linux")]
fn install_ca() -> Result<(), HelperError> {
    let authority = ca::ensure(&mitm_dir()?)?;
    let target = Path::new(LINUX_TRUST_DIR).join(LINUX_TRUST_NAME);
    std::fs::copy(&authority.certificate, target).map_err(HelperError::Write)?;
    run_trust_update()
}

#[cfg(target_os = "linux")]
fn uninstall_ca() -> Result<(), HelperError> {
    let target = Path::new(LINUX_TRUST_DIR).join(LINUX_TRUST_NAME);
    if target.exists() {
        std::fs::remove_file(target).map_err(HelperError::Write)?;
    }
    run_trust_update()
}

#[cfg(target_os = "linux")]
fn run_trust_update() -> Result<(), HelperError> {
    let status = Command::new(UPDATE_CA_CERTIFICATES)
        .status()
        .map_err(HelperError::Write)?;
    if status.success() {
        Ok(())
    } else {
        Err(HelperError::Write(std::io::Error::other(
            "update-ca-certificates failed",
        )))
    }
}

#[cfg(not(target_os = "linux"))]
fn install_ca() -> Result<(), HelperError> {
    Err(HelperError::UnknownTool(
        "system trust installation is only implemented on Linux".to_owned(),
    ))
}

#[cfg(not(target_os = "linux"))]
fn uninstall_ca() -> Result<(), HelperError> {
    Err(HelperError::UnknownTool(
        "system trust removal is only implemented on Linux".to_owned(),
    ))
}
