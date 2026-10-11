//! Just enough XML to read a 3MF model part.
//!
//! A 3MF model is a regular document of elements and attributes: the
//! meshes are thousands of `<vertex x=… y=… z=…/>`, and what matters is
//! the element names, their attributes, and which namespace each belongs
//! to, since the core specification and its extensions share element
//! names and tell them apart only by namespace. This reads exactly that
//! (start and end tags, attributes with their entity escapes resolved,
//! namespace prefixes resolved to their URIs) and skips text, comments,
//! processing instructions, CDATA and the document type, none of which a
//! model part carries meaning in.

use std::borrow::Cow;

use ogeom_core::{OgeomResult, ogeom_bail};

/// The namespace `xml:` is bound to without a declaration.
const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// An element opened, with its attributes; `empty` when it closes itself.
#[derive(Debug)]
pub(crate) struct Start<'a> {
    /// The namespace URI, empty for none.
    pub namespace: Cow<'a, str>,
    /// The name without its prefix.
    pub local: &'a str,
    /// `(namespace, local name, value)`. An unprefixed attribute is in no
    /// namespace, whatever the element's default.
    pub attributes: Vec<(Cow<'a, str>, &'a str, Cow<'a, str>)>,
    /// Whether the element is `<… />`, which has no end event.
    pub empty: bool,
}

impl Start<'_> {
    /// The value of an unprefixed attribute.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(ns, local, _)| ns.is_empty() && *local == name)
            .map(|(_, _, v)| v.as_ref())
    }

    /// The value of an attribute in a namespace.
    pub fn get_in(&self, namespace: &str, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(ns, local, _)| ns == namespace && *local == name)
            .map(|(_, _, v)| v.as_ref())
    }
}

/// One step through the document.
#[derive(Debug)]
pub(crate) enum Event<'a> {
    /// An element opened.
    Start(Start<'a>),
    /// An element closed, by `</…>` or by being empty.
    End,
}

/// A pull reader over one document.
pub(crate) struct Reader<'a> {
    text: &'a str,
    at: usize,
    /// Each open element's namespace declarations, `(prefix, uri)`, the
    /// default namespace under the empty prefix.
    scopes: Vec<Vec<(&'a str, Cow<'a, str>)>>,
    /// An empty element's end, still to be returned after its start.
    owed_end: bool,
}

impl<'a> Reader<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self {
            text: text.strip_prefix('\u{feff}').unwrap_or(text),
            at: 0,
            scopes: Vec::new(),
            owed_end: false,
        }
    }

    /// The next start or end, or `None` at the end of the document.
    pub(crate) fn next(&mut self) -> OgeomResult<Option<Event<'a>>> {
        if self.owed_end {
            self.owed_end = false;
            self.scopes.pop();
            return Ok(Some(Event::End));
        }
        let bytes = self.text.as_bytes();
        loop {
            let Some(offset) = memchr(b'<', &bytes[self.at..]) else {
                return Ok(None);
            };
            self.at += offset;
            let rest = &self.text[self.at..];
            if rest.starts_with("<!--") {
                self.skip_past("-->")?;
            } else if rest.starts_with("<![CDATA[") {
                self.skip_past("]]>")?;
            } else if rest.starts_with("<?") {
                self.skip_past("?>")?;
            } else if rest.starts_with("<!") {
                // A document type; an internal subset would need its own
                // bracket matching, which no model part has.
                self.skip_past(">")?;
            } else if rest.starts_with("</") {
                self.skip_past(">")?;
                if self.scopes.pop().is_none() {
                    ogeom_bail!(Construction, "the XML closes an element it never opened");
                }
                return Ok(Some(Event::End));
            } else {
                return self.start().map(|s| Some(Event::Start(s)));
            }
        }
    }

    /// Skip the rest of the element just opened, children and all.
    pub(crate) fn skip(&mut self) -> OgeomResult<()> {
        let mut depth = 1_usize;
        while depth > 0 {
            match self.next()? {
                Some(Event::Start(_)) => depth += 1,
                Some(Event::End) => depth -= 1,
                None => ogeom_bail!(Construction, "the XML ends inside an element"),
            }
        }
        Ok(())
    }

    fn skip_past(&mut self, marker: &str) -> OgeomResult<()> {
        match self.text[self.at..].find(marker) {
            Some(i) => {
                self.at += i + marker.len();
                Ok(())
            }
            None => ogeom_bail!(Construction, "the XML ends inside a {marker} construct"),
        }
    }

    fn start(&mut self) -> OgeomResult<Start<'a>> {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut at = self.at + 1;
        let name_start = at;
        while at < bytes.len() && !is_space(bytes[at]) && bytes[at] != b'>' && bytes[at] != b'/' {
            at += 1;
        }
        let name = &text[name_start..at];
        let mut raw: Vec<(&'a str, &'a str)> = Vec::new();
        let empty;
        loop {
            while at < bytes.len() && is_space(bytes[at]) {
                at += 1;
            }
            match bytes.get(at) {
                None => ogeom_bail!(Construction, "the XML ends inside the tag {name}"),
                Some(b'>') => {
                    empty = false;
                    at += 1;
                    break;
                }
                Some(b'/') => {
                    if bytes.get(at + 1) != Some(&b'>') {
                        ogeom_bail!(Construction, "the tag {name} has a stray slash");
                    }
                    empty = true;
                    at += 2;
                    break;
                }
                Some(_) => {}
            }
            let key_start = at;
            while at < bytes.len() && bytes[at] != b'=' && !is_space(bytes[at]) {
                at += 1;
            }
            let key = &text[key_start..at];
            while at < bytes.len() && is_space(bytes[at]) {
                at += 1;
            }
            if bytes.get(at) != Some(&b'=') {
                ogeom_bail!(Construction, "the attribute {key} on {name} has no value");
            }
            at += 1;
            while at < bytes.len() && is_space(bytes[at]) {
                at += 1;
            }
            let quote = match bytes.get(at) {
                Some(&q @ (b'"' | b'\'')) => q,
                _ => ogeom_bail!(Construction, "the attribute {key} on {name} is not quoted"),
            };
            at += 1;
            let Some(len) = memchr(quote, &bytes[at..]) else {
                ogeom_bail!(Construction, "the attribute {key} on {name} is not closed");
            };
            raw.push((key, &text[at..at + len]));
            at += len + 1;
        }
        self.at = at;

        let mut scope = Vec::new();
        for &(key, value) in &raw {
            if key == "xmlns" {
                scope.push(("", unescape(value)));
            } else if let Some(prefix) = key.strip_prefix("xmlns:") {
                scope.push((prefix, unescape(value)));
            }
        }
        self.scopes.push(scope);
        let (prefix, local) = split(name);
        let namespace = self.resolve(prefix, name)?;
        let mut attributes = Vec::with_capacity(raw.len());
        for (key, value) in raw {
            if key == "xmlns" || key.starts_with("xmlns:") {
                continue;
            }
            let (prefix, local) = split(key);
            let namespace = if prefix.is_empty() {
                Cow::Borrowed("")
            } else {
                self.resolve(prefix, key)?
            };
            attributes.push((namespace, local, unescape(value)));
        }
        self.owed_end = empty;
        Ok(Start {
            namespace,
            local,
            attributes,
            empty,
        })
    }

    fn resolve(&self, prefix: &str, name: &str) -> OgeomResult<Cow<'a, str>> {
        if prefix == "xml" {
            return Ok(Cow::Borrowed(XML_NAMESPACE));
        }
        for scope in self.scopes.iter().rev() {
            if let Some((_, uri)) = scope.iter().find(|(p, _)| *p == prefix) {
                return Ok(uri.clone());
            }
        }
        if prefix.is_empty() {
            return Ok(Cow::Borrowed(""));
        }
        ogeom_bail!(
            Construction,
            "the XML name {name} uses an undeclared prefix"
        )
    }
}

fn split(name: &str) -> (&str, &str) {
    name.split_once(':').unwrap_or(("", name))
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r')
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

/// Resolve the five named entities and numeric character references.
/// Anything else after an ampersand is left as it stands.
pub(crate) fn unescape(value: &str) -> Cow<'_, str> {
    if !value.contains('&') {
        return Cow::Borrowed(value);
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else {
            break;
        };
        let entity = &rest[1..end];
        let resolved = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match resolved {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::{Event, Reader};

    #[test]
    fn prefixes_resolve_to_their_namespaces() {
        let text = r#"<?xml version="1.0"?><!-- a note --><a xmlns="urn:core" xmlns:p="urn:p"><p:b p:path="/x" n="1 &amp; 2"/></a>"#;
        let mut reader = Reader::new(text);
        let Some(Event::Start(a)) = reader.next().unwrap() else {
            panic!("a start");
        };
        assert_eq!((a.namespace.as_ref(), a.local), ("urn:core", "a"));
        let Some(Event::Start(b)) = reader.next().unwrap() else {
            panic!("a start");
        };
        assert_eq!(
            (b.namespace.as_ref(), b.local, b.empty),
            ("urn:p", "b", true)
        );
        assert_eq!(b.get_in("urn:p", "path"), Some("/x"));
        assert_eq!(b.get("n"), Some("1 & 2"));
        assert!(matches!(reader.next().unwrap(), Some(Event::End)));
        assert!(matches!(reader.next().unwrap(), Some(Event::End)));
        assert!(reader.next().unwrap().is_none());
    }
}
