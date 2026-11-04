use super::*;

pub(super) fn parse_lua_string_map(
    tbl: &LuaTable,
    key: &str,
    context: &str,
) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    let Some(meta) = LuaPluginRuntime::optional_table(tbl, key, context)? else {
        return Ok(map);
    };
    for pair in meta.pairs::<String, String>() {
        let (k, v) = pair
            .map_err(|e| Error::Internal(anyhow!("{context}.{key} must be a string map: {e}")))?;
        map.insert(k, v);
    }
    Ok(map)
}

pub(crate) fn redact_secrets(message: &str) -> String {
    let mut out = message.to_string();
    for marker in ["Authorization:", "authorization:", "Cookie:", "cookie:"] {
        let mut from = 0;
        while let Some(relative) = out[from..].find(marker) {
            let start = from + relative + marker.len();
            let end = out[start..]
                .find(['\r', '\n'])
                .map(|relative| start + relative)
                .unwrap_or(out.len());
            out.replace_range(start..end, " [REDACTED]");
            from = start + " [REDACTED]".len();
        }
    }
    for marker in [
        "access_token=",
        "refresh_token=",
        "authorization=",
        "cookie=",
        "\"access_token\":\"",
        "\"access_token\": \"",
        "\"refresh_token\":\"",
        "\"refresh_token\": \"",
    ] {
        let mut from = 0;
        while let Some(relative) = out[from..].find(marker) {
            let start = from + relative + marker.len();
            let end = out[start..]
                .find(|character: char| {
                    character.is_whitespace()
                        || matches!(character, '&' | ',' | '}' | ']' | '"' | '\'')
                })
                .map(|relative| start + relative)
                .unwrap_or(out.len());
            out.replace_range(start..end, "[REDACTED]");
            from = start + "[REDACTED]".len();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
