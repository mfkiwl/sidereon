#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NtripRejection {
    Unauthorized,
    MountpointNotFound,
    DigestRequired,
    CasterError { reason: String },
    UnexpectedContentType { content_type: String },
    HttpError { status: u16, reason: String },
    MalformedHandshake { prefix: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HttpClassification {
    Stream { chunked: bool },
    Sourcetable { chunked: bool },
    Rejection(NtripRejection),
}

pub fn classify_http_response(
    status: u16,
    reason: &str,
    headers: &[(String, String)],
) -> HttpClassification {
    if status == 401 {
        if digest_required(headers) {
            return HttpClassification::Rejection(NtripRejection::DigestRequired);
        }
        return HttpClassification::Rejection(NtripRejection::Unauthorized);
    }
    if status == 404 {
        return HttpClassification::Rejection(NtripRejection::MountpointNotFound);
    }
    if status != 200 {
        return HttpClassification::Rejection(NtripRejection::HttpError {
            status,
            reason: reason.to_string(),
        });
    }

    let content_type = header_value(headers, "Content-Type").map(media_type);
    match content_type.as_deref() {
        None | Some("gnss/data") => HttpClassification::Stream {
            chunked: transfer_is_chunked(headers),
        },
        Some("gnss/sourcetable") => HttpClassification::Sourcetable {
            chunked: transfer_is_chunked(headers),
        },
        Some(other) => HttpClassification::Rejection(NtripRejection::UnexpectedContentType {
            content_type: other.to_string(),
        }),
    }
}

pub(crate) fn transfer_is_chunked(headers: &[(String, String)]) -> bool {
    header_values(headers, "Transfer-Encoding").any(|value| {
        value
            .split(',')
            .any(|token| token.trim().eq_ignore_ascii_case("chunked"))
    })
}

fn digest_required(headers: &[(String, String)]) -> bool {
    let mut saw = false;
    for value in header_values(headers, "WWW-Authenticate") {
        for challenge in value.split(',') {
            let challenge = challenge.trim_start();
            let Some(scheme) = challenge.split_whitespace().next() else {
                continue;
            };
            if scheme.contains('=') {
                continue;
            }
            saw = true;
            if !scheme.eq_ignore_ascii_case("digest") {
                return false;
            }
        }
    }
    saw
}

fn header_value(headers: &[(String, String)], name: &str) -> Option<String> {
    header_values(headers, name).next().map(str::to_string)
}

fn header_values<'a>(
    headers: &'a [(String, String)],
    name: &'a str,
) -> impl Iterator<Item = &'a str> + 'a {
    headers
        .iter()
        .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn media_type(value: String) -> String {
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}
