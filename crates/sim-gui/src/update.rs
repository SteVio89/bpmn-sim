//! Self-update from GitHub releases: look up the latest release in the background, and on
//! request download the archive for this platform, verify it and swap it in for the running app.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

const LATEST_URL: &str = "https://api.github.com/repos/SteVio89/bpmn-sim/releases/latest";
const USER_AGENT: &str = concat!("bpmn-sim/", env!("CARGO_PKG_VERSION"));
/// Set to any value to skip the check at startup.
const OPT_OUT_ENV: &str = "BPMN_SIM_NO_UPDATE_CHECK";
const MAX_DOWNLOAD: u64 = 300 * 1024 * 1024;
const MAC_BUNDLE: &str = "BPMN Sim.app";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub page: String,
    asset_url: String,
    asset_name: String,
    sha256: Option<String>,
}

#[derive(Default)]
pub enum State {
    #[default]
    Idle,
    Available(Release),
    Installing(Release),
    /// Installed; the new version starts on the next launch.
    Installed(Release),
    Failed(Release, String),
}

enum Message {
    Latest(Option<Release>),
    Installed(Result<(), String>),
}

#[derive(Default)]
pub struct Updater {
    pub state: State,
    rx: Option<Receiver<Message>>,
    /// Where the app was started from; after a swap `current_exe` may no longer be reliable.
    exe: Option<PathBuf>,
}

impl Updater {
    /// Look for a newer release on a worker thread. Offline or rate-limited just means no banner.
    pub fn check(&mut self, ctx: &egui::Context) {
        if std::env::var_os(OPT_OUT_ENV).is_some() {
            return;
        }
        self.exe = std::env::current_exe().ok();
        remove_leftovers(self.exe.as_deref());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let latest = fetch_latest(env!("CARGO_PKG_VERSION"), target())
                .ok()
                .flatten();
            let _ = tx.send(Message::Latest(latest));
            ctx.request_repaint();
        });
    }

    pub fn install(&mut self, ctx: &egui::Context) {
        let (State::Available(r) | State::Failed(r, _)) = std::mem::take(&mut self.state) else {
            return;
        };
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.state = State::Installing(r.clone());
        let (ctx, exe) = (ctx.clone(), self.exe.clone());
        std::thread::spawn(move || {
            let res = download(&r).and_then(|archive| {
                let exe = exe.ok_or_else(|| anyhow!("cannot locate the running program"))?;
                install_archive(&archive, r.asset_name.ends_with(".zip"), &exe)
            });
            let _ = tx.send(Message::Installed(res.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    pub fn dismiss(&mut self) {
        self.state = State::Idle;
    }

    /// Start the freshly installed version; the caller then closes this window.
    pub fn restart(&self) -> Result<()> {
        let exe = self.exe.as_ref().context("unknown executable path")?;
        #[cfg(target_os = "macos")]
        if let Some(bundle) = mac_bundle(exe) {
            std::process::Command::new("open")
                .arg("-n")
                .arg(bundle)
                .spawn()?;
            return Ok(());
        }
        std::process::Command::new(exe).spawn()?;
        Ok(())
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.rx else { return };
        while let Ok(m) = rx.try_recv() {
            match (m, std::mem::take(&mut self.state)) {
                (Message::Latest(Some(r)), State::Idle) => self.state = State::Available(r),
                (Message::Installed(Ok(())), State::Installing(r)) => {
                    self.state = State::Installed(r)
                }
                (Message::Installed(Err(e)), State::Installing(r)) => {
                    self.state = State::Failed(r, e)
                }
                (_, s) => self.state = s,
            }
        }
    }
}

/// The release target name used in the asset file names, if releases exist for this platform.
fn target() -> Option<&'static str> {
    Some(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        _ => return None,
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(300)))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

fn fetch_latest(current: &str, target: Option<&str>) -> Result<Option<Release>> {
    let Some(target) = target else {
        return Ok(None);
    };
    let body = agent()
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .call()?
        .body_mut()
        .read_to_string()?;
    parse_release(&serde_json::from_str(&body)?, current, target)
}

/// The release described by GitHub's JSON, if it is newer than `current` and has our asset.
fn parse_release(json: &serde_json::Value, current: &str, target: &str) -> Result<Option<Release>> {
    let tag = json["tag_name"].as_str().context("release without tag")?;
    let version = tag.trim_start_matches('v');
    match (parse_version(version), parse_version(current)) {
        (Some(latest), Some(cur)) if latest > cur => {}
        (Some(_), Some(_)) => return Ok(None),
        _ => bail!("cannot compare versions {version} and {current}"),
    }
    let ext = if target.contains("windows") {
        "zip"
    } else {
        "tar.gz"
    };
    let wanted = format!("-{target}.{ext}");
    let Some(asset) = json["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(&wanted)))
    else {
        return Ok(None);
    };
    Ok(Some(Release {
        version: version.to_string(),
        page: json["html_url"].as_str().unwrap_or_default().to_string(),
        asset_url: asset["browser_download_url"]
            .as_str()
            .context("asset without URL")?
            .into(),
        asset_name: asset["name"].as_str().unwrap_or_default().to_string(),
        sha256: asset["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_lowercase),
    }))
}

fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let mut parts = v.split('.').map(|p| p.parse().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

fn download(r: &Release) -> Result<Vec<u8>> {
    let mut resp = agent().get(&r.asset_url).call()?;
    let mut data = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_DOWNLOAD)
        .read_to_end(&mut data)?;
    if let Some(expected) = &r.sha256 {
        let actual: String = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if &actual != expected {
            bail!(
                "checksum mismatch for {} (expected {expected}, got {actual})",
                r.asset_name
            );
        }
    }
    Ok(data)
}

/// Unpack the release archive and replace the installed program with what it contains.
fn install_archive(archive: &[u8], zip: bool, exe: &Path) -> Result<()> {
    let bundle = mac_bundle(exe);
    let install_dir = match &bundle {
        Some(b) => b.parent(),
        None => exe.parent(),
    }
    .context("no install directory")?;
    // Next to the target so the final renames stay on one filesystem.
    let staging = install_dir.join(".bpmn-sim-update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir(&staging)
        .with_context(|| format!("cannot write to {}", install_dir.display()))?;
    let result =
        unpack(archive, zip, &staging).and_then(|()| swap_in(&staging, exe, bundle.as_deref()));
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn unpack(archive: &[u8], zip: bool, dir: &Path) -> Result<()> {
    if zip {
        zip::ZipArchive::new(std::io::Cursor::new(archive))?.extract(dir)?;
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        tar.set_preserve_permissions(true);
        tar.unpack(dir)?;
    }
    Ok(())
}

fn swap_in(staging: &Path, exe: &Path, bundle: Option<&Path>) -> Result<()> {
    // The archive holds one folder: bpmn-sim-<tag>-<target>/.
    let root = std::fs::read_dir(staging)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .context("empty archive")?;
    if let Some(bundle) = bundle {
        let new = root.join(MAC_BUNDLE);
        if !new.is_dir() {
            bail!("archive has no {MAC_BUNDLE}");
        }
        // Replacing just the binary would break the bundle's code signature.
        return replace(bundle, &new);
    }
    let name = exe.file_name().context("no executable name")?;
    let new = root.join(name);
    if !new.is_file() {
        bail!("archive has no {}", name.to_string_lossy());
    }
    replace(exe, &new)?;
    // The command-line tool travels with the app if it was installed next to it.
    let cli = format!("sim-cli{}", std::env::consts::EXE_SUFFIX);
    let (old_cli, new_cli) = (exe.with_file_name(&cli), root.join(&cli));
    if old_cli.is_file() && new_cli.is_file() {
        replace(&old_cli, &new_cli)?;
    }
    Ok(())
}

/// Move `new` into the place of `old`. A running executable can be renamed on every platform
/// but not overwritten on Windows, so the old one is moved aside first and cleaned up later.
fn replace(old: &Path, new: &Path) -> Result<()> {
    let aside = leftover(old);
    remove_any(&aside);
    std::fs::rename(old, &aside).with_context(|| format!("cannot replace {}", old.display()))?;
    if let Err(e) = std::fs::rename(new, old) {
        let _ = std::fs::rename(&aside, old);
        return Err(e).with_context(|| format!("cannot install {}", old.display()));
    }
    remove_any(&aside);
    Ok(())
}

fn leftover(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".old");
    s.into()
}

fn remove_any(p: &Path) {
    if p.is_dir() {
        let _ = std::fs::remove_dir_all(p);
    } else {
        let _ = std::fs::remove_file(p);
    }
}

/// Windows cannot delete the old executable while it runs, so tidy up on the next start.
fn remove_leftovers(exe: Option<&Path>) {
    if let Some(exe) = exe {
        remove_any(&leftover(exe));
        remove_any(&leftover(&exe.with_file_name(format!(
            "sim-cli{}",
            std::env::consts::EXE_SUFFIX
        ))));
    }
}

/// `BPMN Sim.app` when the executable is `BPMN Sim.app/Contents/MacOS/sim-gui`.
fn mac_bundle(exe: &Path) -> Option<PathBuf> {
    let bundle = exe.parent()?.parent()?.parent()?;
    let layout =
        exe.parent()?.file_name()? == "MacOS" && exe.parent()?.parent()?.file_name()? == "Contents";
    (layout && bundle.extension()? == "app").then(|| bundle.to_path_buf())
}

/// Banner above the tabs; returns nothing, acts on the updater directly.
pub fn banner(app: &mut crate::app::App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let u = &mut app.updater;
    let (release, msg) = match &u.state {
        State::Idle => return,
        State::Available(r) => (r.clone(), format!("Version {} is available.", r.version)),
        State::Installing(r) => (r.clone(), format!("Installing version {}…", r.version)),
        State::Installed(r) => (r.clone(), format!("Version {} is installed.", r.version)),
        State::Failed(r, e) => (r.clone(), format!("Update to {} failed: {e}", r.version)),
    };
    egui::Panel::top("update").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(msg);
            match &u.state {
                State::Available(_) | State::Failed(..) => {
                    if ui.button("Install update").clicked() {
                        u.install(&ctx);
                    }
                }
                State::Installing(_) => {
                    ui.spinner();
                }
                State::Installed(_) => {
                    if ui.button("Restart now").clicked() {
                        match u.restart() {
                            Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                            Err(e) => u.state = State::Failed(release.clone(), format!("{e:#}")),
                        }
                    }
                }
                State::Idle => {}
            }
            if !release.page.is_empty() {
                ui.hyperlink_to("Release notes", &release.page);
            }
            if !matches!(u.state, State::Installing(_)) && ui.button("Dismiss").clicked() {
                u.dismiss();
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release_json(tag: &str) -> serde_json::Value {
        let asset = |n: &str| {
            json!({"name": format!("bpmn-sim-{tag}-{n}"), "digest": "sha256:ABC",
                   "browser_download_url": format!("https://example.test/{n}")})
        };
        json!({"tag_name": tag, "html_url": "https://example.test/rel", "assets": [
            asset("aarch64-apple-darwin.tar.gz"),
            asset("x86_64-unknown-linux-gnu.tar.gz"),
            asset("x86_64-pc-windows-msvc.zip"),
        ]})
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3-rc1"), None);
    }

    #[test]
    fn picks_the_asset_of_this_platform() {
        let r = parse_release(&release_json("v0.3.0"), "0.2.0", "x86_64-pc-windows-msvc")
            .unwrap()
            .unwrap();
        assert_eq!(r.version, "0.3.0");
        assert_eq!(
            r.asset_url,
            "https://example.test/x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(r.sha256.as_deref(), Some("abc"));
    }

    #[test]
    fn ignores_current_older_and_foreign_releases() {
        let j = release_json("v0.2.0");
        assert!(
            parse_release(&j, "0.2.0", "x86_64-unknown-linux-gnu")
                .unwrap()
                .is_none()
        );
        assert!(
            parse_release(&j, "0.3.0", "x86_64-unknown-linux-gnu")
                .unwrap()
                .is_none()
        );
        let j = release_json("v0.3.0");
        assert!(
            parse_release(&j, "0.2.0", "x86_64-apple-darwin")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn detects_the_mac_bundle() {
        let exe = Path::new("/Applications/BPMN Sim.app/Contents/MacOS/sim-gui");
        assert_eq!(
            mac_bundle(exe),
            Some(PathBuf::from("/Applications/BPMN Sim.app"))
        );
        assert_eq!(mac_bundle(Path::new("/opt/bpmn-sim/sim-gui")), None);
    }

    fn tarball(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, data) in files {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, path, *data).unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar.into_inner().unwrap()).unwrap();
        gz.finish().unwrap()
    }

    #[test]
    fn replaces_binary_and_sibling_cli() {
        let dir = std::env::temp_dir().join(format!("bpmn-sim-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cli = format!("sim-cli{}", std::env::consts::EXE_SUFFIX);
        let gui = format!("sim-gui{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(dir.join(&gui), "old gui").unwrap();
        std::fs::write(dir.join(&cli), "old cli").unwrap();
        let archive = tarball(&[
            (&format!("bpmn-sim-v9/{gui}"), b"new gui"),
            (&format!("bpmn-sim-v9/{cli}"), b"new cli"),
            ("bpmn-sim-v9/README.md", b"readme"),
        ]);
        install_archive(&archive, false, &dir.join(&gui)).unwrap();
        assert_eq!(std::fs::read(dir.join(&gui)).unwrap(), b"new gui");
        assert_eq!(std::fs::read(dir.join(&cli)).unwrap(), b"new cli");
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(names.len(), 2, "no staging or .old leftovers: {names:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_archive_leaves_the_install_untouched() {
        let dir = std::env::temp_dir().join(format!("bpmn-sim-update-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let gui = dir.join(format!("sim-gui{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&gui, "old gui").unwrap();
        assert!(install_archive(b"not an archive", false, &gui).is_err());
        assert_eq!(std::fs::read(&gui).unwrap(), b"old gui");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Talks to GitHub: `cargo test -p sim-gui -- --ignored live_release`.
    #[test]
    #[ignore]
    fn live_release_downloads_and_verifies() {
        let r = fetch_latest("0.0.1", Some("x86_64-unknown-linux-gnu"))
            .unwrap()
            .unwrap();
        assert!(r.sha256.is_some());
        let data = download(&r).unwrap();
        let dir = std::env::temp_dir().join(format!("bpmn-sim-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        unpack(&data, r.asset_name.ends_with(".zip"), &dir).unwrap();
        assert!(std::fs::read_dir(&dir).unwrap().next().is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
