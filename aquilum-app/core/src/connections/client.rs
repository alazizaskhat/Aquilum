use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::{
    io::Read,
    path::Path,
    sync::Mutex,
    time::{Duration, Instant},
};
use url::Url;

pub const PAGE: usize = 20;
pub const PREVIEW: usize = 4000;

pub fn address(value: &str) -> Result<Url, String> {
    let mut url = Url::parse(value.trim()).map_err(|_| "Enter the other PC's HTTPS API address")?;
    let local = url
        .host_str()
        .is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]");
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "Use HTTPS (HTTP is allowed only on this PC), without credentials, query or fragment"
                .into(),
        );
    }
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url)
}

pub struct Client {
    base: Url,
    token: String,
    agent: ureq::Agent,
    last: Mutex<Option<Instant>>,
}

#[derive(Deserialize)]
pub struct Listing {
    pub notes: Vec<String>,
    pub total: usize,
    #[serde(rename = "nextOffset")]
    pub next: Option<usize>,
}

#[derive(Clone, Default, Deserialize)]
pub struct Note {
    pub path: String,
    pub content: String,
    #[serde(default, rename = "nextOffset")]
    pub next: Option<usize>,
}

impl Client {
    pub fn new(endpoint: &str, file: &Path) -> Result<Self, String> {
        let base = address(endpoint)?;
        let mut token = String::new();
        std::fs::File::open(file)
            .map_err(|_| "Cannot open the client token file")?
            .take(1024)
            .read_to_string(&mut token)
            .map_err(|_| "Cannot read the client token file")?;
        let token = token.trim().to_owned();
        if token.len() != 68
            || !token.starts_with("aqg_")
            || !token[4..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("Choose a client .token file created on the other PC".into());
        }
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(25))
            .redirects(0)
            .build();
        Ok(Self {
            base,
            token,
            agent,
            last: Mutex::new(None),
        })
    }

    fn get<T: DeserializeOwned>(
        &self,
        route: &str,
        params: &[(&str, String)],
    ) -> Result<T, String> {
        let mut url = self.base.join(route).map_err(|_| "Invalid API address")?;
        url.query_pairs_mut()
            .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
        // Stay below the backend's default 120 requests/minute during large copies.
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(at) = *last {
            std::thread::sleep(Duration::from_millis(600).saturating_sub(at.elapsed()));
        }
        *last = Some(Instant::now());
        drop(last);
        let response = self.agent.get(url.as_str()).set("Authorization", &format!("Bearer {}", self.token)).call()
            .map_err(|e| match e {
                ureq::Error::Status(401 | 403, _) => "Access refused. The token may have been revoked.".to_owned(),
                ureq::Error::Status(429, _) => "The other PC is busy. Wait a moment and retry.".to_owned(),
                ureq::Error::Status(404, _) => "Not found. Check the API address and update the host for note copying.".to_owned(),
                ureq::Error::Status(code, _) => format!("The other PC returned HTTP {code}"),
                _ => "Cannot connect. Check the address and that the other PC and Aquilum are running.".to_owned(),
            })?;
        if response.status() != 200 {
            return Err("The API must return HTTP 200; redirects are not followed".into());
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(52 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Download interrupted")?;
        if bytes.len() > 52 * 1024 * 1024 {
            return Err("Response is too large".into());
        }
        serde_json::from_slice(&bytes)
            .map_err(|_| "Unexpected API response. Check the API address.".into())
    }

    pub fn list(&self, query: &str, offset: usize) -> Result<Listing, String> {
        let mut params = vec![("offset", offset.to_string()), ("limit", PAGE.to_string())];
        if !query.trim().is_empty() {
            params.push(("q", query.trim().to_owned()));
        }
        self.get("notes", &params)
    }

    pub fn read(&self, path: &str, offset: usize) -> Result<Note, String> {
        let note: Note = self.get(
            "note",
            &[
                ("path", path.into()),
                ("offset", offset.to_string()),
                ("limit", PREVIEW.to_string()),
            ],
        )?;
        let count = note.content.chars().count();
        if note.path != path
            || count > PREVIEW
            || note
                .next
                .is_some_and(|next| count == 0 || next != offset.saturating_add(count))
        {
            return Err("The other PC returned an invalid note page".into());
        }
        Ok(note)
    }

    pub fn export(&self, path: &str) -> Result<Note, String> {
        let note: Note = self.get("export", &[("path", path.into())])?;
        if note.path != path || note.next.is_some() || note.content.len() > 8 * 1024 * 1024 {
            return Err("Invalid note snapshot".into());
        }
        Ok(note)
    }
}

pub fn note_path(value: &str) -> Result<(), String> {
    if value.len() > 2048
        || !value.to_ascii_lowercase().ends_with(".md")
        || value.chars().any(char::is_control)
        || value.contains(['\\', ':', '%', '?', '*', '"', '<', '>', '|'])
        || value.split('/').any(|p| {
            p.is_empty() || p.starts_with('.') || p.ends_with(['.', ' ']) || p != p.trim() || {
                let name = p
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(' ')
                    .to_ascii_uppercase();
                [
                    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6",
                    "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
                    "LPT8", "LPT9", "CONIN$", "CONOUT$", "COM¹", "COM²", "COM³", "LPT¹", "LPT²",
                    "LPT³",
                ]
                .contains(&name.as_str())
            }
        })
    {
        return Err("The other PC returned an unsafe note path".into());
    }
    Ok(())
}
