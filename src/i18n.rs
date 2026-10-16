//! The launcher's translation catalogs (`i18n/*.tsv`, the same files the desktop app uses),
//! parsed once and served to the browser as JSON.
//!
//! Row format: `context<TAB>English<TAB>translation`. An empty context is a plain UI string; `@id`,
//! `@plural` and `@msg` are command-id, plural-form and message rows; any other context is a
//! disambiguation (app taglines by id, month names). Escapes `\t` `\n` `\\` are resolved here.

use serde_json::{Map, Value, json};
use std::{collections::HashMap, sync::OnceLock};

/// (code, native name, TSV source). English needs no catalog.
const LANGS: [(&str, &str, &str); 10] = [
    ("ru", "Русский", include_str!("../i18n/ru.tsv")),
    ("zh-CN", "简体中文", include_str!("../i18n/zh-CN.tsv")),
    ("de", "Deutsch", include_str!("../i18n/de.tsv")),
    ("fr", "Français", include_str!("../i18n/fr.tsv")),
    ("pt-BR", "Português (Brasil)", include_str!("../i18n/pt-BR.tsv")),
    ("es", "Español", include_str!("../i18n/es.tsv")),
    ("cs", "Čeština", include_str!("../i18n/cs.tsv")),
    ("pl", "Polski", include_str!("../i18n/pl.tsv")),
    ("hu", "Magyar", include_str!("../i18n/hu.tsv")),
    ("ja", "日本語", include_str!("../i18n/ja.tsv")),
];

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn parse(code: &str, name: &str, src: &str) -> Value {
    let mut plain = Map::new();
    let mut ctx: Map<String, Value> = Map::new();
    let mut by_id = Map::new();
    let mut plural = Map::new();
    let mut msg = Map::new();
    for line in src.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        let [c, s, t] = parts.as_slice() else { continue };
        let (c, s, t) = (unescape(c), unescape(s), unescape(t));
        match c.as_str() {
            "" => {
                plain.insert(s, Value::String(t));
            }
            "@id" => {
                by_id.insert(s, Value::String(t));
            }
            "@plural" => {
                let forms: Vec<Value> = t.split('|').map(|f| Value::String(f.to_string())).collect();
                if forms.len() <= 3 {
                    plural.insert(s, Value::Array(forms));
                }
            }
            "@msg" => {
                msg.insert(s, Value::String(t));
            }
            other => {
                ctx.entry(other.to_string()).or_insert_with(|| Value::Object(Map::new())).as_object_mut().map(|m| m.insert(s, Value::String(t)));
            }
        }
    }
    json!({ "code": code, "name": name, "plain": plain, "ctx": ctx, "id": by_id, "plural": plural, "msg": msg })
}

fn catalogs() -> &'static HashMap<&'static str, String> {
    static C: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    C.get_or_init(|| LANGS.iter().map(|(code, name, src)| (*code, parse(code, name, src).to_string())).collect())
}

/// `[{code, name}]`, English first.
pub fn list() -> Value {
    let mut v = vec![json!({ "code": "en", "name": "English" })];
    v.extend(LANGS.iter().map(|(c, n, _)| json!({ "code": c, "name": n })));
    Value::Array(v)
}

/// The catalog JSON for a language code, `None` for English or an unknown code.
pub fn catalog(code: &str) -> Option<&'static str> {
    catalogs().get(code).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_parses_and_has_the_core_rows() {
        for (code, _, _) in LANGS {
            let v: Value = serde_json::from_str(catalog(code).unwrap()).unwrap();
            assert!(v["plain"]["Settings"].is_string(), "{code}: Settings");
            assert!(v["plain"]["Update to {v}"].as_str().unwrap().contains("{v}"), "{code}");
            assert!(v["plural"]["{n} app|{n} apps"].is_array(), "{code}");
            assert!(v["ctx"]["photocraft"].is_object() || v["ctx"]["month"].is_object(), "{code}");
        }
        assert!(catalog("en").is_none());
    }
}
