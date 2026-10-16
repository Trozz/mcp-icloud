//! Scripted, stateful fake IMAP server for tests. Plain TCP, one task per connection.
//! Implements only what mcp-icloud-mail sends, with iCloud-like mailbox names.

use mail_parser::MimeHeaders;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone)]
pub struct FakeMessage {
    pub uid: u32,
    pub flags: Vec<String>,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct FakeMailbox {
    pub attrs: Vec<String>,
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub messages: Vec<FakeMessage>,
}

#[derive(Debug, Default)]
pub struct FakeState {
    pub mailboxes: BTreeMap<String, FakeMailbox>,
    pub capabilities: Vec<String>,
    /// One-shot failures: command word (LOGIN, SELECT, STATUS, LIST, SEARCH, FETCH, STORE, MOVE,
    /// COPY, EXPUNGE, APPEND, CREATE) -> text sent back as `TAG NO <text>`.
    pub fail: HashMap<String, String>,
    /// Every command line received, without the tag.
    pub log: Vec<String>,
    pub password: String,
    /// Close the connection (no response) the next time this command word arrives. One-shot.
    pub drop_on: Option<String>,
    /// Send a completion with a foreign tag (`A999 OK stale`) before the real reply the next time
    /// this command word arrives. One-shot.
    pub stale_tag_on: Option<String>,
}

impl FakeState {
    pub fn icloud_like() -> Self {
        let mut s = FakeState {
            capabilities: vec!["MOVE".into(), "UIDPLUS".into(), "SPECIAL-USE".into()],
            password: "app-pass".into(),
            ..Default::default()
        };
        for (name, attr) in [
            ("INBOX", ""),
            ("Sent Messages", "\\Sent"),
            ("Drafts", "\\Drafts"),
            ("Deleted Messages", "\\Trash"),
            ("Junk", "\\Junk"),
            ("Archive", "\\Archive"),
        ] {
            s.mailboxes.insert(
                name.to_string(),
                FakeMailbox {
                    attrs: if attr.is_empty() {
                        vec![]
                    } else {
                        vec![attr.to_string()]
                    },
                    uidvalidity: 1000,
                    uidnext: 1,
                    messages: vec![],
                },
            );
        }
        s
    }

    pub fn add_message(&mut self, mailbox: &str, raw: &[u8], flags: &[&str]) -> u32 {
        let mb = self.mailboxes.get_mut(mailbox).expect("mailbox exists");
        let uid = mb.uidnext;
        mb.uidnext += 1;
        mb.messages.push(FakeMessage {
            uid,
            flags: flags.iter().map(|f| f.to_string()).collect(),
            raw: raw.to_vec(),
        });
        uid
    }
}

pub struct FakeImap {
    pub addr: String,
    pub state: Arc<Mutex<FakeState>>,
}

impl FakeImap {
    pub async fn start(state: FakeState) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let state = Arc::new(Mutex::new(state));
        let st = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(handle(stream, st.clone()));
            }
        });
        FakeImap { addr, state }
    }

    pub fn messages(&self, mailbox: &str) -> Vec<FakeMessage> {
        self.state
            .lock()
            .unwrap()
            .mailboxes
            .get(mailbox)
            .map(|m| m.messages.clone())
            .unwrap_or_default()
    }

    pub fn log(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }
}

async fn handle(stream: TcpStream, state: Arc<Mutex<FakeState>>) {
    let (r, mut w) = stream.into_split();
    let mut r = BufReader::new(r);
    if w.write_all(b"* OK [CAPABILITY IMAP4rev1] fake imap ready\r\n")
        .await
        .is_err()
    {
        return;
    }
    let mut selected: Option<String> = None;
    loop {
        let mut line = String::new();
        match r.read_line(&mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        let (tag, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
        let tag = tag.to_string();
        let mut rest = rest.to_string();
        let mut literal = None;
        if rest.ends_with('}') {
            if let Some(start) = rest.rfind('{') {
                let n: usize = rest[start + 1..rest.len() - 1]
                    .trim_end_matches('+')
                    .parse()
                    .unwrap_or(0);
                if w.write_all(b"+ go ahead\r\n").await.is_err() {
                    return;
                }
                let mut buf = vec![0u8; n];
                if r.read_exact(&mut buf).await.is_err() {
                    return;
                }
                let mut tail = String::new();
                let _ = r.read_line(&mut tail).await;
                literal = Some(buf);
                rest.truncate(start);
            }
        }
        let rest = rest.trim_end().to_string();
        let (out, close) = {
            let mut st = state.lock().unwrap();
            st.log.push(rest.clone());
            let word = command_word(&rest);
            let stale = if st.stale_tag_on.as_deref() == Some(word.as_str()) {
                st.stale_tag_on = None;
                b"A999 OK stale\r\n".to_vec()
            } else {
                Vec::new()
            };
            let (out, close) = if st.drop_on.as_deref() == Some(word.as_str()) {
                st.drop_on = None;
                (Vec::new(), true)
            } else if let Some(msg) = st.fail.remove(&word) {
                (format!("{tag} NO {msg}\r\n").into_bytes(), false)
            } else {
                respond(&mut st, &mut selected, &tag, &rest, literal)
            };
            ([stale, out].concat(), close)
        };
        if !out.is_empty() && w.write_all(&out).await.is_err() {
            return;
        }
        if close {
            return;
        }
    }
}

fn command_word(rest: &str) -> String {
    let mut it = rest.split_whitespace();
    let first = it.next().unwrap_or("").to_ascii_uppercase();
    if first == "UID" {
        return it.next().unwrap_or("").to_ascii_uppercase();
    }
    if first == "EXAMINE" {
        return "SELECT".into();
    }
    first
}

/// Splits into atoms, quoted strings (unquoted, unescaped) and single "(" / ")" tokens.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' => {
                chars.next();
            }
            '(' | ')' => {
                out.push(c.to_string());
                chars.next();
            }
            '"' => {
                chars.next();
                let mut v = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(n) = chars.next() {
                                v.push(n);
                            }
                        }
                        '"' => break,
                        _ => v.push(c),
                    }
                }
                out.push(v);
            }
            _ => {
                let mut v = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ' ' || c == '(' || c == ')' {
                        break;
                    }
                    v.push(c);
                    chars.next();
                }
                out.push(v);
            }
        }
    }
    out
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn nstr(s: Option<&str>) -> String {
    s.map(q).unwrap_or_else(|| "NIL".into())
}

fn in_set(set: &str, uid: u32, max: u32) -> bool {
    set.split(',').any(|part| {
        let parse = |p: &str| {
            if p == "*" {
                max
            } else {
                p.parse().unwrap_or(0)
            }
        };
        match part.split_once(':') {
            Some((a, b)) => {
                let (a, b) = (parse(a), parse(b));
                uid >= a.min(b) && uid <= a.max(b)
            }
            None => parse(part) == uid,
        }
    })
}

fn header_section(raw: &[u8]) -> String {
    let s = String::from_utf8_lossy(raw);
    let end = s
        .find("\r\n\r\n")
        .or_else(|| s.find("\n\n"))
        .unwrap_or(s.len());
    s[..end]
        .replace("\r\n ", " ")
        .replace("\r\n\t", " ")
        .replace("\n ", " ")
}

fn header_contains(raw: &[u8], name: &str, value: &str) -> bool {
    let prefix = format!("{}:", name.to_ascii_lowercase());
    let value = value.to_ascii_lowercase();
    header_section(raw).lines().any(|l| {
        l.to_ascii_lowercase().starts_with(&prefix) && l.to_ascii_lowercase().contains(&value)
    })
}

fn has_flag(m: &FakeMessage, f: &str) -> bool {
    m.flags.iter().any(|x| x.eq_ignore_ascii_case(f))
}

fn eval_key(t: &[String], i: &mut usize, m: &FakeMessage, max: u32) -> bool {
    let Some(key) = t.get(*i).cloned() else {
        return true;
    };
    *i += 1;
    let mut arg = || {
        let v = t.get(*i).cloned().unwrap_or_default();
        *i += 1;
        v
    };
    match key.to_ascii_uppercase().as_str() {
        "ALL" => true,
        "SEEN" => has_flag(m, "\\Seen"),
        "UNSEEN" => !has_flag(m, "\\Seen"),
        "FLAGGED" => has_flag(m, "\\Flagged"),
        "UNFLAGGED" => !has_flag(m, "\\Flagged"),
        "DELETED" => has_flag(m, "\\Deleted"),
        "UNDELETED" => !has_flag(m, "\\Deleted"),
        "DRAFT" => has_flag(m, "\\Draft"),
        "FROM" | "TO" | "CC" | "BCC" | "SUBJECT" => {
            let v = arg();
            header_contains(&m.raw, &key, &v)
        }
        "TEXT" | "BODY" => {
            let v = arg().to_ascii_lowercase();
            String::from_utf8_lossy(&m.raw)
                .to_ascii_lowercase()
                .contains(&v)
        }
        "HEADER" => {
            let name = arg();
            let v = arg();
            header_contains(&m.raw, &name, &v)
        }
        "SINCE" | "BEFORE" | "ON" | "SENTSINCE" | "SENTBEFORE" => {
            arg();
            true
        }
        "CHARSET" => {
            arg();
            true
        }
        "UID" => {
            let set = arg();
            in_set(&set, m.uid, max)
        }
        "NOT" => !eval_key(t, i, m, max),
        "OR" => {
            let a = eval_key(t, i, m, max);
            let b = eval_key(t, i, m, max);
            a || b
        }
        "(" => {
            let mut all = true;
            while t.get(*i).is_some_and(|x| x != ")") {
                all &= eval_key(t, i, m, max);
            }
            *i += 1;
            all
        }
        _ => true,
    }
}

fn matches_search(criteria: &str, m: &FakeMessage, max: u32) -> bool {
    let t = tokens(criteria);
    let mut i = 0;
    let mut all = true;
    while i < t.len() {
        all &= eval_key(&t, &mut i, m, max);
    }
    all
}

fn addr_list(a: Option<&mail_parser::Address<'_>>) -> String {
    let Some(a) = a else { return "NIL".into() };
    let items: Vec<String> = a
        .iter()
        .map(|x| {
            let email = x.address().unwrap_or("");
            let (mb, host) = email.split_once('@').unwrap_or((email, ""));
            format!("({} NIL {} {})", nstr(x.name()), q(mb), q(host))
        })
        .collect();
    if items.is_empty() {
        "NIL".into()
    } else {
        format!("({})", items.join(""))
    }
}

fn envelope(raw: &[u8]) -> String {
    let Some(m) = mail_parser::MessageParser::default().parse(raw) else {
        return "(NIL NIL NIL NIL NIL NIL NIL NIL NIL NIL)".into();
    };
    let raw_header = |name: &str| m.header_raw(name).map(|v| v.trim().to_string());
    let from = addr_list(m.from());
    format!(
        "({} {} {} {} {} {} {} {} {} {})",
        nstr(raw_header("Date").as_deref()),
        nstr(raw_header("Subject").as_deref()),
        from,
        from,
        if m.reply_to().is_some() {
            addr_list(m.reply_to())
        } else {
            from.clone()
        },
        addr_list(m.to()),
        addr_list(m.cc()),
        addr_list(m.bcc()),
        nstr(raw_header("In-Reply-To").as_deref()),
        nstr(raw_header("Message-ID").as_deref()),
    )
}

fn bodystructure(raw: &[u8]) -> String {
    let text = "(\"TEXT\" \"PLAIN\" (\"CHARSET\" \"UTF-8\") NIL NIL \"7BIT\" 10 1)";
    let mixed = mail_parser::MessageParser::default()
        .parse(raw)
        .and_then(|m| {
            m.root_part().content_type().map(|c| {
                c.ctype().eq_ignore_ascii_case("multipart")
                    && c.subtype().is_some_and(|s| s.eq_ignore_ascii_case("mixed"))
            })
        })
        .unwrap_or(false);
    if mixed {
        format!("({text}(\"APPLICATION\" \"OCTET-STREAM\" NIL NIL NIL \"BASE64\" 10) \"MIXED\")")
    } else {
        text.to_string()
    }
}

fn fetch_response(seq: usize, m: &FakeMessage, items: &str) -> Vec<u8> {
    let items = items.to_ascii_uppercase();
    let mut parts = vec![format!("UID {}", m.uid)];
    if items.contains("FLAGS") {
        parts.push(format!("FLAGS ({})", m.flags.join(" ")));
    }
    if items.contains("INTERNALDATE") {
        parts.push("INTERNALDATE \"15-Oct-2026 10:00:00 +0000\"".into());
    }
    if items.contains("RFC822.SIZE") {
        parts.push(format!("RFC822.SIZE {}", m.raw.len()));
    }
    if items.contains("ENVELOPE") {
        parts.push(format!("ENVELOPE {}", envelope(&m.raw)));
    }
    if items.contains("BODYSTRUCTURE") {
        parts.push(format!("BODYSTRUCTURE {}", bodystructure(&m.raw)));
    }
    let mut out = format!("* {seq} FETCH ({}", parts.join(" ")).into_bytes();
    if items.contains("BODY.PEEK[]") || items.contains("BODY[]") {
        out.extend_from_slice(format!(" BODY[] {{{}}}\r\n", m.raw.len()).as_bytes());
        out.extend_from_slice(&m.raw);
    }
    out.extend_from_slice(b")\r\n");
    out
}

fn ok(tag: &str) -> Vec<u8> {
    format!("{tag} OK done\r\n").into_bytes()
}

fn no(tag: &str, msg: &str) -> Vec<u8> {
    format!("{tag} NO {msg}\r\n").into_bytes()
}

fn bad(tag: &str, msg: &str) -> Vec<u8> {
    format!("{tag} BAD {msg}\r\n").into_bytes()
}

fn move_or_copy(
    st: &mut FakeState,
    from: &str,
    set: &str,
    target: &str,
    remove: bool,
) -> Result<Vec<u8>, String> {
    if !st.mailboxes.contains_key(target) {
        return Err("[TRYCREATE] no such mailbox".into());
    }
    let src = st.mailboxes.get_mut(from).ok_or("no mailbox selected")?;
    let max = src.uidnext.saturating_sub(1);
    let mut moved = Vec::new();
    let mut expunged_seqs = Vec::new();
    for (idx, m) in src.messages.iter().enumerate() {
        if in_set(set, m.uid, max) {
            moved.push(m.clone());
            expunged_seqs.push(idx + 1);
        }
    }
    if remove {
        src.messages.retain(|m| !in_set(set, m.uid, max));
    }
    let src_uv = src.uidvalidity;
    let dst = st.mailboxes.get_mut(target).expect("checked");
    let mut new_uids = Vec::new();
    for mut m in moved.clone() {
        m.uid = dst.uidnext;
        dst.uidnext += 1;
        new_uids.push(m.uid);
        dst.messages.push(m);
    }
    let _ = src_uv;
    let mut out = String::new();
    if !moved.is_empty() {
        let old: Vec<String> = moved.iter().map(|m| m.uid.to_string()).collect();
        let new: Vec<String> = new_uids.iter().map(|u| u.to_string()).collect();
        out.push_str(&format!(
            "* OK [COPYUID {} {} {}] copied\r\n",
            dst.uidvalidity,
            old.join(","),
            new.join(",")
        ));
    }
    if remove {
        for seq in expunged_seqs.iter().rev() {
            out.push_str(&format!("* {seq} EXPUNGE\r\n"));
        }
    }
    Ok(out.into_bytes())
}

fn respond(
    st: &mut FakeState,
    selected: &mut Option<String>,
    tag: &str,
    rest: &str,
    literal: Option<Vec<u8>>,
) -> (Vec<u8>, bool) {
    let t = tokens(rest);
    let cmd = t
        .first()
        .map(|s| s.to_ascii_uppercase())
        .unwrap_or_default();
    let caps = st.capabilities.join(" ");
    let out = match cmd.as_str() {
        "CAPABILITY" => format!("* CAPABILITY IMAP4rev1 {caps}\r\n{tag} OK done\r\n").into_bytes(),
        "NOOP" => ok(tag),
        "LOGOUT" => {
            return (
                format!("* BYE logging out\r\n{tag} OK done\r\n").into_bytes(),
                true,
            );
        }
        "LOGIN" => {
            if t.get(2).map(String::as_str) == Some(st.password.as_str()) {
                format!("{tag} OK [CAPABILITY IMAP4rev1 {caps}] logged in\r\n").into_bytes()
            } else {
                no(tag, "[AUTHENTICATIONFAILED] Authentication Failed")
            }
        }
        "LIST" => {
            let mut out = String::new();
            for (name, mb) in &st.mailboxes {
                let mut attrs = vec!["\\HasNoChildren".to_string()];
                attrs.extend(mb.attrs.iter().cloned());
                out.push_str(&format!(
                    "* LIST ({}) \"/\" {}\r\n",
                    attrs.join(" "),
                    q(name)
                ));
            }
            out.push_str(&format!("{tag} OK done\r\n"));
            out.into_bytes()
        }
        "STATUS" => {
            let name = t.get(1).cloned().unwrap_or_default();
            match st.mailboxes.get(&name) {
                Some(mb) => {
                    let unseen = mb
                        .messages
                        .iter()
                        .filter(|m| !has_flag(m, "\\Seen"))
                        .count();
                    format!(
                        "* STATUS {} (MESSAGES {} UNSEEN {} UIDNEXT {} UIDVALIDITY {})\r\n{tag} OK done\r\n",
                        q(&name),
                        mb.messages.len(),
                        unseen,
                        mb.uidnext,
                        mb.uidvalidity
                    )
                    .into_bytes()
                }
                None => no(tag, "[NONEXISTENT] no such mailbox"),
            }
        }
        "SELECT" | "EXAMINE" => {
            let name = t.get(1).cloned().unwrap_or_default();
            match st.mailboxes.get(&name) {
                Some(mb) => {
                    *selected = Some(name);
                    format!(
                        "* {} EXISTS\r\n* 0 RECENT\r\n* FLAGS (\\Answered \\Flagged \\Deleted \\Seen \\Draft)\r\n* OK [UIDVALIDITY {}] UIDs valid\r\n* OK [UIDNEXT {}] next\r\n{tag} OK [READ-WRITE] done\r\n",
                        mb.messages.len(),
                        mb.uidvalidity,
                        mb.uidnext
                    )
                    .into_bytes()
                }
                None => {
                    *selected = None;
                    no(tag, "[NONEXISTENT] no such mailbox")
                }
            }
        }
        "CREATE" => {
            let name = t.get(1).cloned().unwrap_or_default();
            if st.mailboxes.contains_key(&name) {
                no(tag, "[ALREADYEXISTS] mailbox exists")
            } else {
                st.mailboxes.insert(
                    name,
                    FakeMailbox {
                        attrs: vec![],
                        uidvalidity: 2000,
                        uidnext: 1,
                        messages: vec![],
                    },
                );
                ok(tag)
            }
        }
        "APPEND" => {
            let name = t.get(1).cloned().unwrap_or_default();
            let mut flags = Vec::new();
            if t.get(2).map(String::as_str) == Some("(") {
                let mut i = 3;
                while t.get(i).is_some_and(|x| x != ")") {
                    flags.push(t[i].clone());
                    i += 1;
                }
            }
            let uidplus = st.capabilities.iter().any(|c| c == "UIDPLUS");
            match (st.mailboxes.get_mut(&name), literal) {
                (Some(mb), Some(raw)) => {
                    let uid = mb.uidnext;
                    mb.uidnext += 1;
                    mb.messages.push(FakeMessage { uid, flags, raw });
                    if uidplus {
                        format!("{tag} OK [APPENDUID {} {uid}] appended\r\n", mb.uidvalidity)
                            .into_bytes()
                    } else {
                        ok(tag)
                    }
                }
                (None, _) => no(tag, "[TRYCREATE] no such mailbox"),
                (_, None) => bad(tag, "missing literal"),
            }
        }
        "EXPUNGE" => {
            let Some(name) = selected.clone() else {
                return (bad(tag, "no mailbox selected"), false);
            };
            let mb = st.mailboxes.get_mut(&name).expect("selected exists");
            let mut out = String::new();
            let seqs: Vec<usize> = mb
                .messages
                .iter()
                .enumerate()
                .filter(|(_, m)| has_flag(m, "\\Deleted"))
                .map(|(i, _)| i + 1)
                .collect();
            for s in seqs.iter().rev() {
                out.push_str(&format!("* {s} EXPUNGE\r\n"));
            }
            mb.messages.retain(|m| !has_flag(m, "\\Deleted"));
            out.push_str(&format!("{tag} OK done\r\n"));
            out.into_bytes()
        }
        "UID" => {
            let Some(name) = selected.clone() else {
                return (bad(tag, "no mailbox selected"), false);
            };
            let sub = t.get(1).map(|s| s.to_ascii_uppercase()).unwrap_or_default();
            let after_sub = rest.splitn(3, ' ').nth(2).unwrap_or("").to_string();
            match sub.as_str() {
                "SEARCH" => {
                    let mb = &st.mailboxes[&name];
                    let max = mb.uidnext.saturating_sub(1);
                    let uids: Vec<String> = mb
                        .messages
                        .iter()
                        .filter(|m| matches_search(&after_sub, m, max))
                        .map(|m| m.uid.to_string())
                        .collect();
                    let sep = if uids.is_empty() { "" } else { " " };
                    format!("* SEARCH{sep}{}\r\n{tag} OK done\r\n", uids.join(" ")).into_bytes()
                }
                "FETCH" => {
                    let set = t.get(2).cloned().unwrap_or_default();
                    let items = after_sub.splitn(2, ' ').nth(1).unwrap_or("").to_string();
                    let mb = &st.mailboxes[&name];
                    let max = mb.uidnext.saturating_sub(1);
                    let mut out = Vec::new();
                    for (idx, m) in mb.messages.iter().enumerate() {
                        if in_set(&set, m.uid, max) {
                            out.extend(fetch_response(idx + 1, m, &items));
                        }
                    }
                    out.extend(ok(tag));
                    out
                }
                "STORE" => {
                    let set = t.get(2).cloned().unwrap_or_default();
                    let op = t.get(3).cloned().unwrap_or_default().to_ascii_uppercase();
                    let silent = op.ends_with(".SILENT");
                    let flags: Vec<String> = t
                        .iter()
                        .skip(4)
                        .filter(|x| *x != "(" && *x != ")")
                        .cloned()
                        .collect();
                    let mb = st.mailboxes.get_mut(&name).expect("selected exists");
                    let max = mb.uidnext.saturating_sub(1);
                    let mut out = Vec::new();
                    for (idx, m) in mb.messages.iter_mut().enumerate() {
                        if !in_set(&set, m.uid, max) {
                            continue;
                        }
                        for f in &flags {
                            if op.starts_with('+')
                                && !m.flags.iter().any(|x| x.eq_ignore_ascii_case(f))
                            {
                                m.flags.push(f.clone());
                            }
                            if op.starts_with('-') {
                                m.flags.retain(|x| !x.eq_ignore_ascii_case(f));
                            }
                        }
                        if !silent {
                            out.extend(
                                format!(
                                    "* {} FETCH (UID {} FLAGS ({}))\r\n",
                                    idx + 1,
                                    m.uid,
                                    m.flags.join(" ")
                                )
                                .into_bytes(),
                            );
                        }
                    }
                    out.extend(ok(tag));
                    out
                }
                "MOVE" | "COPY" => {
                    if sub == "MOVE" && !st.capabilities.iter().any(|c| c == "MOVE") {
                        return (bad(tag, "unknown command"), false);
                    }
                    let set = t.get(2).cloned().unwrap_or_default();
                    let target = t.get(3).cloned().unwrap_or_default();
                    match move_or_copy(st, &name, &set, &target, sub == "MOVE") {
                        Ok(mut out) => {
                            out.extend(ok(tag));
                            out
                        }
                        Err(e) => no(tag, &e),
                    }
                }
                "EXPUNGE" => {
                    if !st.capabilities.iter().any(|c| c == "UIDPLUS") {
                        return (bad(tag, "unknown command"), false);
                    }
                    let set = t.get(2).cloned().unwrap_or_default();
                    let mb = st.mailboxes.get_mut(&name).expect("selected exists");
                    let max = mb.uidnext.saturating_sub(1);
                    let mut out = String::new();
                    let seqs: Vec<usize> = mb
                        .messages
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| has_flag(m, "\\Deleted") && in_set(&set, m.uid, max))
                        .map(|(i, _)| i + 1)
                        .collect();
                    for s in seqs.iter().rev() {
                        out.push_str(&format!("* {s} EXPUNGE\r\n"));
                    }
                    mb.messages
                        .retain(|m| !(has_flag(m, "\\Deleted") && in_set(&set, m.uid, max)));
                    out.push_str(&format!("{tag} OK done\r\n"));
                    out.into_bytes()
                }
                _ => bad(tag, "unknown UID command"),
            }
        }
        _ => bad(tag, "unknown command"),
    };
    (out, false)
}
