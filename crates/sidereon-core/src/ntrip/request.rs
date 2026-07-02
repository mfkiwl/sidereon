use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NtripVersion {
    Rev1,
    Rev2,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NtripCredentials {
    pub username: String,
    pub password: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NtripConfig {
    pub host: String,
    pub port: u16,
    pub mountpoint: String,
    pub version: NtripVersion,
    pub credentials: Option<NtripCredentials>,
    pub user_agent_product: String,
    pub gga_interval_s: Option<f64>,
}

impl Default for NtripConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 2101,
            mountpoint: String::new(),
            version: NtripVersion::Rev2,
            credentials: None,
            user_agent_product: format!("sidereon/{}", env!("CARGO_PKG_VERSION")),
            gga_interval_s: None,
        }
    }
}

impl NtripConfig {
    pub fn request_bytes(&self) -> Result<Vec<u8>> {
        let path = self.validated_path()?;
        let headers = self.common_headers()?;
        let mut out = Vec::new();
        match self.version {
            NtripVersion::Rev1 => {
                write_line(&mut out, &format!("GET {path} HTTP/1.0"));
                for (name, value) in headers {
                    write_line(&mut out, &format!("{name}: {value}"));
                }
            }
            NtripVersion::Rev2 => {
                write_line(&mut out, &format!("GET {path} HTTP/1.1"));
                for (name, value) in headers {
                    write_line(&mut out, &format!("{name}: {value}"));
                }
            }
        }
        out.extend_from_slice(b"\r\n");
        Ok(out)
    }

    pub fn request_headers(&self) -> Result<(String, Vec<(String, String)>)> {
        if self.version != NtripVersion::Rev2 {
            return Err(Error::InvalidInput(
                "request_headers is only defined for NTRIP rev2".into(),
            ));
        }
        Ok((self.validated_path()?, self.common_headers()?))
    }

    fn validated_path(&self) -> Result<String> {
        validate_config(self)?;
        if self.mountpoint.is_empty() {
            Ok("/".into())
        } else {
            Ok(format!("/{}", self.mountpoint))
        }
    }

    fn common_headers(&self) -> Result<Vec<(String, String)>> {
        validate_config(self)?;
        let mut headers = Vec::new();
        if self.version == NtripVersion::Rev2 {
            headers.push(("Host".into(), format!("{}:{}", self.host, self.port)));
            headers.push(("Ntrip-Version".into(), "Ntrip/2.0".into()));
        }
        headers.push((
            "User-Agent".into(),
            format!("NTRIP {}", self.user_agent_product),
        ));
        if let Some(credentials) = &self.credentials {
            let token = format!("{}:{}", credentials.username, credentials.password);
            headers.push((
                "Authorization".into(),
                format!("Basic {}", base64(token.as_bytes())),
            ));
        }
        if self.version == NtripVersion::Rev2 {
            headers.push(("Connection".into(), "close".into()));
        }
        Ok(headers)
    }
}

fn validate_config(config: &NtripConfig) -> Result<()> {
    if config
        .mountpoint
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || b == b'/' || b == b'?')
    {
        return Err(Error::InvalidInput(
            "NTRIP mountpoint contains a forbidden byte".into(),
        ));
    }

    let product = &config.user_agent_product;
    let slash_count = product.bytes().filter(|&b| b == b'/').count();
    if product.is_empty()
        || slash_count != 1
        || product
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(Error::InvalidInput(
            "user_agent_product must be name/version with no whitespace".into(),
        ));
    }

    if let Some(credentials) = &config.credentials {
        if credentials.username.contains(':') {
            return Err(Error::InvalidInput(
                "NTRIP username must not contain ':'".into(),
            ));
        }
        if credentials
            .username
            .bytes()
            .chain(credentials.password.bytes())
            .any(|b| b == b'\r' || b == b'\n')
        {
            return Err(Error::InvalidInput(
                "NTRIP credentials must not contain CR or LF".into(),
            ));
        }
    }

    if let Some(interval) = config.gga_interval_s {
        if !interval.is_finite() || interval <= 0.0 {
            return Err(Error::InvalidInput(
                "gga_interval_s must be finite and positive".into(),
            ));
        }
    }

    Ok(())
}

fn write_line(out: &mut Vec<u8>, line: &str) {
    out.extend_from_slice(line.as_bytes());
    out.extend_from_slice(b"\r\n");
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((u32::from(b0)) << 16) | ((u32::from(b1)) << 8) | u32::from(b2);
        out.push(ALPHABET[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}
