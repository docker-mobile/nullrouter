use std::{fs, path::Path};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn workspace_root() -> TestResult<&'static Path> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            std::io::Error::other("contracts crate is not under the workspace root").into()
        })
}

fn read_manifest(path: &Path) -> TestResult<toml::Value> {
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

fn string_array<'a>(value: &'a toml::Value, path: &[&str]) -> TestResult<Vec<&'a str>> {
    let value = path.iter().try_fold(value, |current, key| {
        current
            .get(*key)
            .ok_or_else(|| std::io::Error::other(format!("missing TOML key {key}")))
    })?;
    value
        .as_array()
        .ok_or_else(|| std::io::Error::other("TOML value is not an array"))?
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| std::io::Error::other("TOML array item is not a string").into())
        })
        .collect()
}

#[test]
fn workspace_registers_nullrouter_auth_package_and_binary() -> TestResult {
    let root = workspace_root()?;
    let workspace = read_manifest(&root.join("Cargo.toml"))?;
    let auth = read_manifest(&root.join("services/auth-actix/Cargo.toml"))?;

    let members = string_array(&workspace, &["workspace", "members"])?;
    let defaults = string_array(&workspace, &["workspace", "default-members"])?;
    assert!(members.contains(&"services/auth-actix"));
    assert!(defaults.contains(&"services/auth-actix"));
    assert_eq!(
        auth.get("package")
            .and_then(|value| value.get("name"))
            .and_then(toml::Value::as_str),
        Some("nullrouter-auth")
    );
    let binary_names = auth
        .get("bin")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| std::io::Error::other("auth manifest has no [[bin]] entries"))?
        .iter()
        .filter_map(|binary| binary.get("name").and_then(toml::Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(binary_names, ["nullrouter-auth"]);
    Ok(())
}

#[test]
fn auth_package_uses_root_workspace_and_lockfile() -> TestResult {
    let root = workspace_root()?;
    let auth = read_manifest(&root.join("services/auth-actix/Cargo.toml"))?;

    assert!(auth.get("workspace").is_none());
    assert!(!root.join("services/auth-actix/Cargo.lock").exists());
    Ok(())
}
