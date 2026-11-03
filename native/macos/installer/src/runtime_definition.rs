//! Exact account-bound definition for the Driver-owned engine; no arbitrary root commands.
use crate::ServiceAccount;
use anyhow::{Context as _, Result, ensure};
use plist::Value;

const TEMPLATE: &str =
    include_str!("../../../../packaging/macos/io.mixrelay.audio.engine.plist.in");

pub(crate) fn from_template(bytes: &[u8], account: &ServiceAccount) -> Result<Value> {
    ensure!(bytes.len() <= 16 * 1024, "engine definition too large");
    let supplied = Value::from_reader(std::io::Cursor::new(bytes))?;
    ensure!(
        supplied == Value::from_reader_xml(TEMPLATE.as_bytes())?,
        "invalid engine launch template"
    );
    for_account(account)
}

pub(crate) fn for_account(account: &ServiceAccount) -> Result<Value> {
    let mut value = Value::from_reader_xml(TEMPLATE.as_bytes())?;
    let dict = value
        .as_dictionary_mut()
        .context("engine definition is not a dictionary")?;
    let home = account.home().to_str().context("home must be UTF-8")?;
    dict.insert("UserName".into(), Value::String(account.user().into()));
    dict.insert("GroupName".into(), Value::String(account.group().into()));
    let environment = dict
        .get_mut("EnvironmentVariables")
        .and_then(Value::as_dictionary_mut)
        .context("engine environment missing")?;
    for (key, value) in [
        ("HOME", home),
        ("USER", account.user()),
        ("LOGNAME", account.user()),
    ] {
        environment.insert(key.into(), Value::String(value.into()));
    }
    for (key, name) in [
        ("StandardOutPath", "stdout"),
        ("StandardErrorPath", "stderr"),
    ] {
        dict.insert(
            key.into(),
            Value::String(format!("{home}/Library/Logs/MixRelay/engine.{name}.log")),
        );
    }
    Ok(value)
}

pub(crate) fn validate_installed(bytes: &[u8], account: &ServiceAccount) -> Result<Value> {
    ensure!(bytes.len() <= 16 * 1024, "engine definition too large");
    let value = Value::from_reader(std::io::Cursor::new(bytes))?;
    ensure!(
        value == for_account(account)?,
        "invalid installed engine definition"
    );
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn engine_runs_as_user_and_rejects_a_modified_definition() {
        let account =
            ServiceAccount::new("listener".into(), "staff".into(), "/Users/listener".into())
                .unwrap();
        let mut value = for_account(&account).unwrap();
        assert_eq!(
            value.as_dictionary().unwrap()["UserName"].as_string(),
            Some("listener")
        );
        assert_eq!(
            value.as_dictionary().unwrap()["AssociatedBundleIdentifiers"].as_string(),
            Some("io.mixrelay.desktop")
        );
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes).unwrap();
        assert!(validate_installed(&bytes, &account).is_ok());
        value.as_dictionary_mut().unwrap().insert(
            "ProgramArguments".into(),
            Value::Array(vec![Value::String("/bin/sh".into())]),
        );
        bytes.clear();
        value.to_writer_xml(&mut bytes).unwrap();
        assert!(validate_installed(&bytes, &account).is_err());
        assert!(from_template(&bytes, &account).is_err());
    }

    #[test]
    fn engine_is_on_demand_and_fully_bound_to_the_user() {
        let account = ServiceAccount::new(
            "artist".into(),
            "staff".into(),
            "/Users/artist & studio".into(),
        )
        .unwrap();
        let value = for_account(&account).unwrap();
        let dict = value.as_dictionary().unwrap();
        assert_eq!(dict["KeepAlive"].as_boolean(), Some(false));
        assert_eq!(dict["RunAtLoad"].as_boolean(), Some(false));
        assert_eq!(
            dict["MachServices"].as_dictionary().unwrap()["io.mixrelay.audio.engine"].as_boolean(),
            Some(true)
        );
        let mut bytes = Vec::new();
        value.to_writer_xml(&mut bytes).unwrap();
        assert_eq!(Value::from_reader_xml(bytes.as_slice()).unwrap(), value);
        assert!(!String::from_utf8(bytes).unwrap().contains("__MIXRELAY"));
    }
}
