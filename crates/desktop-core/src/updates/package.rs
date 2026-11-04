//! Package selection from the release support contract and native distribution identity.

use serde::Deserialize;

use super::UpdateError;

const SUPPORT: &str = include_str!("../../../../support/environments.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Package {
    Pkg,
    Deb,
    Rpm,
    Arch,
}

impl Package {
    pub(super) fn accepts(self, url: &str) -> bool {
        let suffix = match self {
            Self::Pkg => ".pkg",
            Self::Deb => ".deb",
            Self::Rpm => ".rpm",
            Self::Arch => ".pkg.tar.zst",
        };
        url::Url::parse(url).is_ok_and(|url| url.path().ends_with(suffix))
    }
}

#[derive(Deserialize)]
struct Support {
    artifacts: Vec<Artifact>,
    #[serde(rename = "qualificationTargets")]
    targets: Vec<Target>,
}

/// The contract also lists artifacts that are not app installers (the Stream Deck
/// plugin), so formats stay strings and only installer formats are offered.
#[derive(Deserialize)]
struct Artifact {
    id: String,
    os: String,
    arch: String,
    format: String,
    asset: String,
}

/// One published release file named by the support contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub os: String,
    pub arch: String,
    /// False for companion files that no platform installs, such as the Stream Deck plugin.
    pub installer: bool,
}

fn support() -> Result<Support, UpdateError> {
    serde_json::from_str(SUPPORT).map_err(|_| UpdateError::Unavailable)
}

pub(super) fn release_assets(version: &str) -> Result<Vec<ReleaseAsset>, UpdateError> {
    Ok(support()?
        .artifacts
        .into_iter()
        .map(|artifact| ReleaseAsset {
            name: artifact.asset.replace("{version}", version),
            os: artifact.os,
            arch: artifact.arch,
            installer: artifact.format != "streamDeckPlugin",
        })
        .collect())
}

#[derive(Deserialize)]
struct Target {
    distro: Option<String>,
    arch: String,
    artifact: String,
}

pub(super) fn linux(os_release: &str, arch: &str) -> Result<Package, UpdateError> {
    let distro = os_release
        .lines()
        .find_map(|line| line.strip_prefix("ID="))
        .map(|id| id.trim_matches(['\'', '"']))
        .ok_or(UpdateError::Unavailable)?;
    let support = support()?;
    let target = support
        .targets
        .iter()
        .find(|target| target.distro.as_deref() == Some(distro) && target.arch == arch)
        .ok_or(UpdateError::Unavailable)?;
    support
        .artifacts
        .iter()
        .find(|artifact| artifact.id == target.artifact)
        .and_then(|artifact| match artifact.format.as_str() {
            "pkg" => Some(Package::Pkg),
            "deb" => Some(Package::Deb),
            "rpm" => Some(Package::Rpm),
            "arch" => Some(Package::Arch),
            _ => None,
        })
        .ok_or(UpdateError::Unavailable)
}

#[cfg(test)]
mod tests;
