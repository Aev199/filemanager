//! Self-update of the portable EXE from GitHub Releases.
//!
//! The manual build publishes a release with `Papka.exe` and `latest.json`
//! (version, commit, SHA-256, change list). The app picks the newest release
//! of its channel, downloads the EXE, checks its hash and swaps files by
//! renaming: Windows lets a running EXE be renamed, not overwritten. The
//! previous EXE stays beside it as `<name>.old.exe` for one-click rollback.

use std::cmp::Ordering;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const REPOSITORY: &str = "Aev199/filemanager";
pub const EXE_ASSET: &str = "Papka.exe";
pub const MANIFEST_ASSET: &str = "latest.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Channel {
    /// Releases marked as final only.
    Stable,
    /// Every build, test ones included.
    #[default]
    Beta,
}

impl Channel {
    pub fn key(self) -> &'static str {
        match self { Channel::Stable => "stable", Channel::Beta => "beta" }
    }
    pub fn from_key(key: &str) -> Channel {
        if key == "stable" { Channel::Stable } else { Channel::Beta }
    }
    pub fn label(self) -> &'static str {
        match self { Channel::Stable => "Стабильные", Channel::Beta => "Все сборки (бета)" }
    }
}

/// "v0.1.44" / "0.1.44-dev" → [0, 1, 44].
pub fn parse_version(text: &str) -> Vec<u64> {
    text.trim().trim_start_matches(['v', 'V']).split(['.', '-', '+']).map_while(|part| part.parse().ok()).collect()
}

pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let (a, b) = (parse_version(a), parse_version(b));
    (0..a.len().max(b.len()))
        .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
        .find(|ordering| ordering.is_ne())
        .unwrap_or(Ordering::Equal)
}

#[derive(Deserialize)]
struct GithubAsset { name: String, browser_download_url: String }

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)] draft: bool,
    #[serde(default)] prerelease: bool,
    #[serde(default)] assets: Vec<GithubAsset>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub version: String,
    pub page: String,
    pub exe_url: String,
    pub manifest_url: String,
    pub prerelease: bool,
}

/// The newest complete release of `channel` in GitHub's release list.
pub fn pick_release(list: &[u8], channel: Channel) -> io::Result<Option<Candidate>> {
    let releases: Vec<GithubRelease> = serde_json::from_slice(list).map_err(io::Error::other)?;
    Ok(releases.into_iter()
        .filter(|release| !release.draft && (channel == Channel::Beta || !release.prerelease))
        .filter_map(|release| {
            let url = |name: &str| release.assets.iter().find(|asset| asset.name == name).map(|asset| asset.browser_download_url.clone());
            Some(Candidate {
                exe_url: url(EXE_ASSET)?,
                manifest_url: url(MANIFEST_ASSET)?,
                version: release.tag_name.trim_start_matches(['v', 'V']).to_string(),
                page: release.html_url,
                prerelease: release.prerelease,
            })
        })
        .max_by(|a, b| compare_versions(&a.version, &b.version)))
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    #[serde(default)] pub commit: String,
    pub sha256: String,
    #[serde(default)] pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    pub version: String,
    pub commit: String,
    pub notes: Vec<String>,
    pub sha256: String,
    pub exe_url: String,
    pub page: String,
    pub prerelease: bool,
}

/// Fetches a URL; the app passes WinHTTP, tests pass a stub.
pub type Fetch<'a> = &'a dyn Fn(&str, &mut dyn FnMut(u64, Option<u64>)) -> io::Result<Vec<u8>>;

pub fn releases_url() -> String {
    format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=20")
}

/// A newer version than `current` on `channel`, if there is one.
pub fn check(current: &str, channel: Channel, fetch: Fetch) -> io::Result<Option<Update>> {
    let list = fetch(&releases_url(), &mut |_, _| {})?;
    let Some(candidate) = pick_release(&list, channel)? else { return Ok(None) };
    if compare_versions(&candidate.version, current) != Ordering::Greater { return Ok(None); }
    let manifest: Manifest = serde_json::from_slice(&fetch(&candidate.manifest_url, &mut |_, _| {})?).map_err(io::Error::other)?;
    if compare_versions(&manifest.version, &candidate.version) != Ordering::Equal || manifest.sha256.len() != 64 {
        return Err(io::Error::other("описание выпуска не совпадает с самим выпуском"));
    }
    Ok(Some(Update {
        version: candidate.version,
        commit: manifest.commit,
        notes: manifest.notes,
        sha256: manifest.sha256.to_ascii_lowercase(),
        exe_url: candidate.exe_url,
        page: candidate.page,
        prerelease: candidate.prerelease,
    }))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Where the replaced EXE is kept for rollback.
pub fn previous_path(exe: &Path) -> PathBuf {
    let stem = exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| "Papka".into());
    exe.with_file_name(format!("{stem}.old.exe"))
}

fn staged_path(exe: &Path) -> PathBuf {
    let stem = exe.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_else(|| "Papka".into());
    exe.with_file_name(format!("{stem}.new.exe"))
}

/// Puts `bytes` in place of `exe` after checking the hash; the old EXE
/// becomes `previous_path(exe)`. Nothing changes if any step fails.
pub fn install(exe: &Path, bytes: &[u8], sha256: &str) -> io::Result<()> {
    if !sha256_hex(bytes).eq_ignore_ascii_case(sha256.trim()) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "контрольная сумма загруженного файла не совпала"));
    }
    if !bytes.starts_with(b"MZ") {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "загруженный файл — не программа Windows"));
    }
    let staged = staged_path(exe);
    let previous = previous_path(exe);
    let written = (|| {
        let mut file = fs::File::create(&staged)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    if previous.exists() {
        if let Err(error) = fs::remove_file(&previous) {
            let _ = fs::remove_file(&staged);
            return Err(error);
        }
    }
    if let Err(error) = fs::rename(exe, &previous) {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    if let Err(error) = fs::rename(&staged, exe) {
        let _ = fs::rename(&previous, exe);
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    Ok(())
}

/// Swaps `exe` with the kept previous version, so it can be undone too.
pub fn rollback(exe: &Path) -> io::Result<()> {
    let previous = previous_path(exe);
    if !previous.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "предыдущей версии нет"));
    }
    let swap = staged_path(exe);
    let _ = fs::remove_file(&swap);
    fs::rename(exe, &swap)?;
    if let Err(error) = fs::rename(&previous, exe) {
        let _ = fs::rename(&swap, exe);
        return Err(error);
    }
    if let Err(error) = fs::rename(&swap, &previous) {
        // The previous version runs either way; only the copy to undo is lost.
        let _ = fs::remove_file(&swap);
        return Err(error);
    }
    Ok(())
}

/// Readable text for WinHTTP's own error codes, which Windows cannot format.
pub fn winhttp_message(code: i32) -> Option<&'static str> {
    Some(match code {
        12002 => "сервер не ответил вовремя",
        12005 | 12006 => "неверный адрес",
        12007 => "сервер не найден: нет интернета или DNS",
        12029 => "не удалось подключиться к серверу: сеть, прокси, брандмауэр или антивирус не пропускают программу",
        12030 | 12031 => "соединение прервано",
        12038 | 12044 | 12045 | 12057 | 12157 | 12175 => "ошибка защищённого соединения (сертификат или TLS); возможно, его подменяет прокси или антивирус",
        12152 => "неверный ответ сервера или прокси",
        12180 => "не удалось определить настройки прокси",
        _ => return None,
    })
}

pub fn describe(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => match winhttp_message(code) {
            Some(text) => format!("{text} (код {code})"),
            None => error.to_string(),
        },
        None => error.to_string(),
    }
}

/// GET over HTTPS. WinHTTP with the system proxy settings first; if the
/// connection itself fails, PowerShell's web client (system proxy and the
/// Windows login for it), which corporate filters often let through.
#[cfg(windows)]
pub fn fetch(url: &str, progress: &mut dyn FnMut(u64, Option<u64>)) -> io::Result<Vec<u8>> {
    match winhttp_get(url, progress) {
        Ok(body) => Ok(body),
        // The server answered: another route would get the same answer.
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Err(error),
        Err(first) => match powershell_get(url) {
            Ok(body) => {
                progress(body.len() as u64, Some(body.len() as u64));
                Ok(body)
            }
            Err(second) => Err(io::Error::other(format!(
                "{}. Запасной способ (PowerShell) тоже не сработал: {second}", describe(&first)))),
        },
    }
}

#[cfg(windows)]
fn winhttp_get(url: &str, progress: &mut dyn FnMut(u64, Option<u64>)) -> io::Result<Vec<u8>> {
    use windows_sys::Win32::Networking::WinHttp::*;
    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) { if !self.0.is_null() { unsafe { WinHttpCloseHandle(self.0); } } }
    }
    fn wide(text: &str) -> Vec<u16> { text.encode_utf16().chain(Some(0)).collect() }
    fn check(handle: *mut core::ffi::c_void) -> io::Result<Handle> {
        if handle.is_null() { Err(io::Error::last_os_error()) } else { Ok(Handle(handle)) }
    }
    let rest = url.strip_prefix("https://").ok_or_else(|| io::Error::other("поддерживается только https"))?;
    let (host, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(host, path)| (host, format!("/{path}")));
    unsafe {
        let agent = wide(&format!("Papka/{}", env!("CARGO_PKG_VERSION")));
        let session = check(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0))?;
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 60_000);
        // TLS 1.2 is not on by default in older Windows builds; 1.3 where known.
        for protocols in [WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2] {
            if WinHttpSetOption(session.0, WINHTTP_OPTION_SECURE_PROTOCOLS, (&protocols as *const u32).cast(), 4) != 0 { break; }
        }
        let connection = check(WinHttpConnect(session.0, wide(host).as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0))?;
        let verb = wide("GET");
        let request = check(WinHttpOpenRequest(connection.0, verb.as_ptr(), wide(&path).as_ptr(), std::ptr::null(),
            std::ptr::null(), std::ptr::null(), WINHTTP_FLAG_SECURE))?;
        // A proxy asking for the Windows login gets it, as in the browser.
        let policy = WINHTTP_AUTOLOGON_SECURITY_LEVEL_LOW;
        WinHttpSetOption(request.0, WINHTTP_OPTION_AUTOLOGON_POLICY, (&policy as *const u32).cast(), 4);
        let headers = wide("Accept: application/vnd.github+json, application/octet-stream\r\n");
        if WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, std::ptr::null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0 {
            return Err(io::Error::last_os_error());
        }
        let number = |query: u32| {
            let mut value = 0u32;
            let mut size = 4u32;
            (WinHttpQueryHeaders(request.0, query | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(),
                (&mut value as *mut u32).cast(), &mut size, std::ptr::null_mut()) != 0).then_some(value)
        };
        let status = number(WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
        match status {
            200 => {}
            // The proxy refused the login: worth the other route.
            407 => return Err(io::Error::new(io::ErrorKind::PermissionDenied, "прокси требует входа (407)")),
            403 | 429 => return Err(io::Error::new(io::ErrorKind::InvalidData, "GitHub временно ограничил запросы, попробуйте через час")),
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, format!("сервер ответил {status}"))),
        }
        let total = number(WINHTTP_QUERY_CONTENT_LENGTH).map(u64::from);
        let mut body = Vec::with_capacity(total.unwrap_or(0).min(64 << 20) as usize);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            if WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read) == 0 {
                return Err(io::Error::last_os_error());
            }
            if read == 0 { break; }
            body.extend_from_slice(&buffer[..read as usize]);
            progress(body.len() as u64, total);
        }
        Ok(body)
    }
}

/// The PowerShell script that downloads `url` into `target`.
pub fn powershell_script(url: &str, target: &Path) -> String {
    let quote = |text: &str| format!("'{}'", text.replace('\'', "''"));
    format!("$ErrorActionPreference='Stop'; \
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12; \
        $c = New-Object Net.WebClient; \
        $c.Headers.Add('User-Agent', 'Papka'); \
        $c.Headers.Add('Accept', 'application/vnd.github+json, application/octet-stream'); \
        $c.Proxy = [Net.WebRequest]::GetSystemWebProxy(); \
        $c.Proxy.Credentials = [Net.CredentialCache]::DefaultNetworkCredentials; \
        $c.DownloadFile({}, {})", quote(url), quote(&target.to_string_lossy()))
}

/// `-EncodedCommand` text: Base64 of UTF-16LE, so no quoting can break it.
pub fn encode_command(script: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16) | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8) | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= chunk.len() { out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char) } else { out.push('=') }
        }
    }
    out
}

#[cfg(windows)]
fn powershell_get(url: &str) -> io::Result<Vec<u8>> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let target = std::env::temp_dir().join(format!("papka-download-{}-{}.tmp", std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand",
            &encode_command(&powershell_script(url, &target))])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let result = match output {
        Ok(output) if output.status.success() => fs::read(&target),
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stderr);
            let line = text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("неизвестная ошибка");
            Err(io::Error::other(line.chars().take(300).collect::<String>()))
        }
        Err(error) => Err(error),
    };
    let _ = fs::remove_file(&target);
    result
}

/// Development builds elsewhere use curl; updates themselves are Windows-only.
#[cfg(not(windows))]
pub fn fetch(url: &str, progress: &mut dyn FnMut(u64, Option<u64>)) -> io::Result<Vec<u8>> {
    let output = std::process::Command::new("curl").args(["-fsSL", "-H", "User-Agent: Papka", url]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_string()));
    }
    progress(output.stdout.len() as u64, Some(output.stdout.len() as u64));
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
        {"tag_name":"v0.1.46","html_url":"h46","draft":true,"prerelease":false,
         "assets":[{"name":"Papka.exe","browser_download_url":"e46"},{"name":"latest.json","browser_download_url":"m46"}]},
        {"tag_name":"v0.1.45","html_url":"h45","draft":false,"prerelease":true,
         "assets":[{"name":"Papka.exe","browser_download_url":"e45"},{"name":"latest.json","browser_download_url":"m45"}]},
        {"tag_name":"v0.1.44","html_url":"h44","draft":false,"prerelease":false,
         "assets":[{"name":"Papka.exe","browser_download_url":"e44"},{"name":"latest.json","browser_download_url":"m44"}]},
        {"tag_name":"v0.1.50","html_url":"h50","draft":false,"prerelease":false,
         "assets":[{"name":"Papka-win64.zip","browser_download_url":"z50"}]}
    ]"#;

    #[test]
    fn powershell_route_is_encoded_safely() {
        // "abc" in UTF-16LE is 61 00 62 00 63 00.
        assert_eq!(encode_command("abc"), "YQBiAGMA");
        assert_eq!(encode_command("a"), "YQA=");
        let script = powershell_script("https://x/y?a='b'", Path::new(r"C:\Temp\O'Neil.tmp"));
        assert!(script.contains(r"'https://x/y?a=''b'''"));
        assert!(script.contains(r"'C:\Temp\O''Neil.tmp'"));
    }

    #[test]
    fn winhttp_codes_read_in_russian() {
        assert!(winhttp_message(12029).unwrap().contains("подключиться"));
        assert!(describe(&io::Error::from_raw_os_error(12029)).ends_with("(код 12029)"));
        assert_eq!(winhttp_message(5), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(compare_versions("0.1.10", "0.1.9"), Ordering::Greater);
        assert_eq!(compare_versions("v0.1.44", "0.1.44"), Ordering::Equal);
        assert_eq!(compare_versions("0.2", "0.1.99"), Ordering::Greater);
        assert_eq!(compare_versions("0.1.0-dev", "0.1.1"), Ordering::Less);
    }

    #[test]
    fn channel_picks_newest_complete_release() {
        let beta = pick_release(LIST.as_bytes(), Channel::Beta).unwrap().unwrap();
        assert_eq!((beta.version.as_str(), beta.exe_url.as_str(), beta.prerelease), ("0.1.45", "e45", true));
        let stable = pick_release(LIST.as_bytes(), Channel::Stable).unwrap().unwrap();
        assert_eq!((stable.version.as_str(), stable.manifest_url.as_str()), ("0.1.44", "m44"));
        assert!(pick_release(b"[]", Channel::Beta).unwrap().is_none());
    }

    fn stub(manifest: &'static str) -> impl Fn(&str, &mut dyn FnMut(u64, Option<u64>)) -> io::Result<Vec<u8>> {
        move |url, _| Ok(if url == releases_url() { LIST.as_bytes().to_vec() } else { manifest.as_bytes().to_vec() })
    }

    #[test]
    fn check_reads_the_manifest_of_a_newer_release() {
        let sha = "a".repeat(64);
        let manifest: &'static str = Box::leak(format!(r#"{{"version":"0.1.45","commit":"abc","sha256":"{sha}","notes":["Поиск"]}}"#).into_boxed_str());
        let found = check("0.1.44", Channel::Beta, &stub(manifest)).unwrap().unwrap();
        assert_eq!((found.version.as_str(), found.notes.as_slice(), found.exe_url.as_str()), ("0.1.45", &["Поиск".to_string()][..], "e45"));
        assert!(check("0.1.45", Channel::Beta, &stub(manifest)).unwrap().is_none());
        assert!(check("0.1.43", Channel::Stable, &stub(manifest)).is_err(), "manifest of another version is refused");
    }

    #[test]
    fn install_swaps_files_and_rollback_swaps_back() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Papka.exe");
        fs::write(&exe, b"MZ old").unwrap();
        let new = b"MZ new".to_vec();
        assert!(install(&exe, &new, &"0".repeat(64)).is_err());
        assert!(install(&exe, b"not an exe", &sha256_hex(b"not an exe")).is_err());
        assert_eq!(fs::read(&exe).unwrap(), b"MZ old");
        install(&exe, &new, &sha256_hex(&new).to_uppercase()).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"MZ new");
        assert_eq!(fs::read(previous_path(&exe)).unwrap(), b"MZ old");
        rollback(&exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"MZ old");
        assert_eq!(fs::read(previous_path(&exe)).unwrap(), b"MZ new");
        assert!(!staged_path(&exe).exists());
    }
}
