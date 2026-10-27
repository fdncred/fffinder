//! Server addresses for Connect to Server: what was typed, made into a URL
//! each system's mounting tool understands.

/// The kinds of server Connect to Server knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Windows file sharing (`smb://`, `cifs://`, or `\\server\share`).
    Smb,
    Sftp,
    /// WebDAV over `https://` (`davs://`) or `http://` (`dav://`).
    WebDav {
        secure: bool,
    },
    Afp,
    Nfs,
    Ftp,
}

/// A server address, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub protocol: Protocol,
    /// `host/share/path`, after the scheme, as typed (user and port kept).
    pub rest: String,
}

impl Server {
    /// Read an address: `smb://nas/share`, `sftp://me@box/srv`,
    /// `https://cloud/dav`, `\\nas\share`, or a bare `nas/share` (SMB).
    ///
    /// # Errors
    ///
    /// An empty address, or a scheme that isn't one of [`Protocol`]'s.
    pub fn parse(text: &str) -> Result<Server, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Type a server address".into());
        }
        if let Some(unc) = text.strip_prefix("\\\\") {
            return Ok(Server {
                protocol: Protocol::Smb,
                rest: unc.replace('\\', "/"),
            });
        }
        let (scheme, rest) = match text.split_once("://") {
            Some((scheme, rest)) => (scheme.to_lowercase(), rest),
            None => ("smb".to_string(), text),
        };
        let protocol = match scheme.as_str() {
            "smb" | "cifs" => Protocol::Smb,
            "sftp" | "ssh" => Protocol::Sftp,
            "https" | "davs" | "webdavs" => Protocol::WebDav { secure: true },
            "http" | "dav" | "webdav" => Protocol::WebDav { secure: false },
            "afp" => Protocol::Afp,
            "nfs" => Protocol::Nfs,
            "ftp" => Protocol::Ftp,
            other => return Err(format!("“{other}://” servers aren’t supported")),
        };
        let rest = rest.trim_end_matches('/').to_string();
        if rest.is_empty() {
            return Err("The address has no server".into());
        }
        Ok(Server { protocol, rest })
    }

    /// The host, without a user or port.
    pub fn host(&self) -> &str {
        let authority = self.rest.split('/').next().unwrap_or_default();
        let host = authority.rsplit('@').next().unwrap_or(authority);
        host.split(':').next().unwrap_or(host)
    }

    /// The URL macOS's `mount volume` takes. WebDAV is `http(s)://` there,
    /// and SFTP isn't supported.
    pub fn mac_url(&self) -> Option<String> {
        let scheme = match self.protocol {
            Protocol::Smb => "smb",
            Protocol::WebDav { secure: true } => "https",
            Protocol::WebDav { secure: false } => "http",
            Protocol::Afp => "afp",
            Protocol::Nfs => "nfs",
            Protocol::Ftp => "ftp",
            Protocol::Sftp => return None,
        };
        Some(format!("{scheme}://{}", self.rest))
    }

    /// The URI GIO's `gio mount` takes (Linux).
    pub fn gio_uri(&self) -> Option<String> {
        let scheme = match self.protocol {
            Protocol::Smb => "smb",
            Protocol::Sftp => "sftp",
            Protocol::WebDav { secure: true } => "davs",
            Protocol::WebDav { secure: false } => "dav",
            Protocol::Afp => "afp",
            Protocol::Nfs => "nfs",
            Protocol::Ftp => "ftp",
        };
        Some(format!("{scheme}://{}", self.rest))
    }

    /// `\\server\share`, which Windows opens without mounting anything.
    pub fn unc_path(&self) -> Option<String> {
        (self.protocol == Protocol::Smb).then(|| format!("\\\\{}", self.rest.replace('/', "\\")))
    }

    /// What `net use` maps to a drive letter on Windows (WebDAV).
    pub fn webdav_url(&self) -> Option<String> {
        match self.protocol {
            Protocol::WebDav { secure } => Some(format!(
                "{}://{}",
                if secure { "https" } else { "http" },
                self.rest
            )),
            _ => None,
        }
    }
}

/// The mount point in `gio info` output: its `local path:` line.
pub fn gio_local_path(info: &str) -> Option<std::path::PathBuf> {
    info.lines()
        .find_map(|line| line.trim().strip_prefix("local path:"))
        .map(|path| std::path::PathBuf::from(path.trim()))
}

/// The drive `net use * <url>` connected: "Drive Z: is now connected…".
pub fn net_use_drive(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|word| {
        let letter = word.strip_suffix(':')?;
        (letter.len() == 1 && letter.chars().all(|c| c.is_ascii_alphabetic()))
            .then(|| format!("{letter}:\\"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses() {
        let smb = Server::parse("nas.local/media").unwrap();
        assert_eq!(smb.protocol, Protocol::Smb);
        assert_eq!(smb.mac_url().as_deref(), Some("smb://nas.local/media"));
        assert_eq!(smb.unc_path().as_deref(), Some("\\\\nas.local\\media"));
        assert_eq!(
            Server::parse("\\\\nas\\media\\tv").unwrap().rest,
            "nas/media/tv"
        );

        let sftp = Server::parse("SFTP://me@box:2222/srv/").unwrap();
        assert_eq!(sftp.protocol, Protocol::Sftp);
        assert_eq!(sftp.host(), "box");
        assert_eq!(sftp.mac_url(), None, "macOS can't mount SFTP");
        assert_eq!(sftp.gio_uri().as_deref(), Some("sftp://me@box:2222/srv"));

        let dav = Server::parse("https://cloud.example.com/remote.php/dav").unwrap();
        assert_eq!(dav.protocol, Protocol::WebDav { secure: true });
        assert_eq!(
            dav.gio_uri().as_deref(),
            Some("davs://cloud.example.com/remote.php/dav")
        );
        assert_eq!(
            dav.webdav_url().as_deref(),
            Some("https://cloud.example.com/remote.php/dav")
        );

        assert!(Server::parse("  ").is_err());
        assert!(Server::parse("gopher://old").is_err());
        assert!(Server::parse("smb://").is_err());
    }

    #[test]
    fn reads_tool_output() {
        let info = "display name: media on nas\nlocal path: /run/user/1000/gvfs/smb-share:server=nas,share=media\nuri: smb://nas/media/\n";
        assert_eq!(
            gio_local_path(info),
            Some("/run/user/1000/gvfs/smb-share:server=nas,share=media".into())
        );
        assert_eq!(
            net_use_drive("Drive Z: is now connected to https://cloud/dav.\r\n"),
            Some("Z:\\".into())
        );
        assert_eq!(net_use_drive("System error 67 has occurred."), None);
    }
}
