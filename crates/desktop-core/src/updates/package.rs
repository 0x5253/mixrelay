//! Package selection from the release support contract and native distribution identity.

use serde::Deserialize;

use super::UpdateError;

const SUPPORT: &str = include_str!("../../../../support/environments.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Package {
    Pkg,
    Deb,
    Arch,
}

impl Package {
    pub(super) fn accepts(self, url: &str) -> bool {
        let suffix = match self {
            Self::Pkg => ".pkg",
            Self::Deb => ".deb",
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
    format: String,
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
    let support: Support = serde_json::from_str(SUPPORT).map_err(|_| UpdateError::Unavailable)?;
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
            "arch" => Some(Package::Arch),
            _ => None,
        })
        .ok_or(UpdateError::Unavailable)
}

#[cfg(test)]
mod tests;
