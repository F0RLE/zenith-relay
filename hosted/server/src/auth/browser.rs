use super::*;
use ring::rand::SecureRandom;
use zeroize::Zeroizing;

pub(crate) type Token = Zeroizing<[u8; 32]>;
pub(super) const ATTEMPT: &str = "__Host-relay_admin_attempt";
pub(super) const SESSION: &str = "__Host-relay_admin";

pub(crate) fn random() -> Option<Token> {
    let mut raw = Token::default();
    SystemRandom::new().fill(&mut *raw).ok()?;
    Some(raw)
}

pub(crate) fn one<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    values.next().is_none().then_some(value)
}

pub(crate) fn same_origin(headers: &HeaderMap, origin: &str) -> bool {
    one(headers, "origin") == Some(origin)
}

pub(crate) fn decode(text: &str) -> Option<Token> {
    let mut value = Token::default();
    hex::decode_to_slice(text, &mut *value).ok()?;
    Some(value)
}

pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<Token> {
    let mut found = None;
    for line in headers.get_all(header::COOKIE) {
        for field in line.to_str().ok()?.split(';') {
            if let Some((key, value)) = field.trim().split_once('=')
                && key == name
            {
                if found.is_some() {
                    return None;
                }
                found = Some(decode(value)?);
            }
        }
    }
    found
}

pub(crate) fn set(r: &mut Response, name: &str, value: Option<&Token>, seconds: u32) {
    set_policy(r, name, value, seconds, "Strict");
}

pub(crate) fn set_policy(
    r: &mut Response,
    name: &str,
    value: Option<&Token>,
    seconds: u32,
    policy: &str,
) {
    let text = Zeroizing::new(value.map_or_else(String::new, |raw| hex::encode(raw.as_ref())));
    let line = Zeroizing::new(format!(
        "{name}={}; Path=/; Max-Age={seconds}; Secure; HttpOnly; SameSite={policy}",
        text.as_str()
    ));
    let mut cookie = HeaderValue::from_str(&line).expect("fixed cookie name and hexadecimal token");
    cookie.set_sensitive(true);
    r.headers_mut().append(header::SET_COOKIE, cookie);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_foreign_or_duplicate_credentials_never_select_an_owner() {
        let mut h = HeaderMap::new();
        let token = "a9".repeat(32);
        let customer = format!("__Host-relay_customer={token}");
        h.insert(header::COOKIE, customer.parse().unwrap());
        assert!(cookie(&h, SESSION).is_none());
        let holder = format!("{SESSION}={token}").parse::<HeaderValue>().unwrap();
        h.append(header::COOKIE, holder.clone());
        assert!(cookie(&h, SESSION).is_some());
        h.append(header::COOKIE, holder);
        assert!(cookie(&h, SESSION).is_none());
        h.append(
            header::ORIGIN,
            HeaderValue::from_static("https://example.test"),
        );
        h.append(
            header::ORIGIN,
            HeaderValue::from_static("https://other.test"),
        );
        assert!(!same_origin(&h, "https://example.test"));
    }
}
