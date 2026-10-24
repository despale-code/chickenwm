//! BitTorrent Engine for `chickenwm-downloader`:
//! - RFC 3174 SHA-1 digest for `.torrent` `info_hash` and piece verification
//! - Full Bencode parser (`i..e`, `<len>:..`, `l..e`, `d..e`) and raw `info` slice locator
//! - `.torrent` metadata parser (single-file and multi-file) & `magnet:?xt=urn:btih:...` parser (hex & base32)
//! - UDP Tracker Protocol (BEP 15) + HTTP/HTTPS Tracker announce
//! - BitTorrent Peer Wire Protocol (BEP 3 / BEP 10) piece downloader with pause/resume & speed limit

use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
];

/// Pure-Rust RFC 3174 SHA-1 implementation (used for 20-byte BitTorrent `info_hash` and piece hashes).
pub fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xEFCD_AB89;
    let mut h2: u32 = 0x98BA_DCFE;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xC3D2_E1F0;

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999_u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1_u32),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC_u32),
                _ => (b ^ c ^ d, 0xCA62_C1D6_u32),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

pub fn hex_decode_20(hex: &str) -> Option<[u8; 20]> {
    let hex = hex.trim();
    if hex.len() != 40 {
        return None;
    }
    let mut out = [0u8; 20];
    for i in 0..20 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// Decodes a 32-character RFC 4648 Base32 `btih` magnet hash into 20 bytes.
pub fn base32_decode_20(s: &str) -> Option<[u8; 20]> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let mut bits: u64 = 0;
    let mut bit_count = 0u32;
    let mut out = [0u8; 20];
    let mut idx = 0usize;
    for ch in s.bytes() {
        let val: u64 = match ch {
            b'A'..=b'Z' => (ch - b'A') as u64,
            b'a'..=b'z' => (ch - b'a') as u64,
            b'2'..=b'7' => (ch - b'2' + 26) as u64,
            _ => return None,
        };
        bits = (bits << 5) | val;
        bit_count += 5;
        while bit_count >= 8 {
            bit_count -= 8;
            if idx >= 20 {
                return None;
            }
            out[idx] = ((bits >> bit_count) & 0xFF) as u8;
            idx += 1;
        }
    }
    (idx == 20).then_some(out)
}

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex_str) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(val) = u8::from_str_radix(hex_str, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
            }
        } else if bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn url_encode_bytes(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 3);
    for &b in bytes {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                s.push(b as char);
            }
            _ => {
                use std::fmt::Write;
                let _ = write!(&mut s, "%{b:02X}");
            }
        }
    }
    s
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BValue {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<BValue>),
    Dict(BTreeMap<Vec<u8>, BValue>),
}

impl BValue {
    pub fn as_int(&self) -> Option<i64> {
        match self {
            BValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            BValue::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<String> {
        self.as_bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    pub fn as_list(&self) -> Option<&[BValue]> {
        match self {
            BValue::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&BTreeMap<Vec<u8>, BValue>> {
        match self {
            BValue::Dict(d) => Some(d),
            _ => None,
        }
    }

    pub fn dict_get(&self, key: &str) -> Option<&BValue> {
        self.as_dict()?.get(key.as_bytes())
    }
}

/// Parses a Bencoded value at `data[pos..]`, returning `(BValue, next_pos, Option<info_raw_range>)`.
pub fn parse_bencode(data: &[u8]) -> Result<(BValue, Option<(usize, usize)>)> {
    let mut info_span = None;
    let (val, _) = parse_bencode_at(data, 0, &mut info_span)?;
    Ok((val, info_span))
}

fn parse_bencode_at(
    data: &[u8],
    pos: usize,
    info_span: &mut Option<(usize, usize)>,
) -> Result<(BValue, usize)> {
    if pos >= data.len() {
        bail!("unexpected end of bencode input");
    }
    match data[pos] {
        b'i' => {
            let end = data[pos + 1..]
                .iter()
                .position(|&b| b == b'e')
                .ok_or_else(|| anyhow!("unterminated bencode int"))?
                + pos
                + 1;
            let s = std::str::from_utf8(&data[pos + 1..end])?;
            let n: i64 = s.parse()?;
            Ok((BValue::Int(n), end + 1))
        }
        b'l' => {
            let mut cur = pos + 1;
            let mut list = Vec::new();
            while cur < data.len() && data[cur] != b'e' {
                let (v, next) = parse_bencode_at(data, cur, info_span)?;
                list.push(v);
                cur = next;
            }
            if cur >= data.len() {
                bail!("unterminated bencode list");
            }
            Ok((BValue::List(list), cur + 1))
        }
        b'd' => {
            let mut cur = pos + 1;
            let mut dict = BTreeMap::new();
            while cur < data.len() && data[cur] != b'e' {
                let (k_val, val_start) = parse_bencode_at(data, cur, info_span)?;
                let key = match k_val {
                    BValue::Bytes(b) => b,
                    _ => bail!("bencode dict key must be byte string"),
                };
                let is_info_key = key == b"info" && info_span.is_none();
                let (v, next) = parse_bencode_at(data, val_start, info_span)?;
                if is_info_key {
                    *info_span = Some((val_start, next));
                }
                dict.insert(key, v);
                cur = next;
            }
            if cur >= data.len() {
                bail!("unterminated bencode dict");
            }
            Ok((BValue::Dict(dict), cur + 1))
        }
        b'0'..=b'9' => {
            let colon = data[pos..]
                .iter()
                .position(|&b| b == b':')
                .ok_or_else(|| anyhow!("missing colon in bencode string"))?
                + pos;
            let len_str = std::str::from_utf8(&data[pos..colon])?;
            let len: usize = len_str.parse()?;
            let start = colon + 1;
            let end = start + len;
            if end > data.len() {
                bail!("bencode string length exceeds input");
            }
            Ok((BValue::Bytes(data[start..end].to_vec()), end))
        }
        other => bail!("invalid bencode token byte: 0x{other:02x}"),
    }
}

#[derive(Debug, Clone)]
pub struct TorrentMeta {
    pub name: String,
    pub info_hash: [u8; 20],
    pub total_size: u64,
    pub piece_length: u64,
    pub piece_hashes: Vec<[u8; 20]>,
    pub trackers: Vec<String>,
}

impl TorrentMeta {
    pub fn info_hash_hex(&self) -> String {
        hex_encode(&self.info_hash)
    }

    pub fn from_torrent_bytes(bytes: &[u8]) -> Result<Self> {
        let (root, info_span) = parse_bencode(bytes)?;
        let (info_start, info_end) =
            info_span.ok_or_else(|| anyhow!("missing `info` dictionary in .torrent file"))?;
        let info_hash = sha1_digest(&bytes[info_start..info_end]);

        let mut trackers = Vec::new();
        if let Some(ann) = root.dict_get("announce").and_then(|v| v.as_str()) {
            if !ann.trim().is_empty() {
                trackers.push(ann.trim().to_string());
            }
        }
        if let Some(tiers) = root.dict_get("announce-list").and_then(|v| v.as_list()) {
            for tier in tiers {
                if let Some(urls) = tier.as_list() {
                    for u in urls {
                        if let Some(s) = u.as_str() {
                            let s = s.trim().to_string();
                            if !s.is_empty() && !trackers.contains(&s) {
                                trackers.push(s);
                            }
                        }
                    }
                }
            }
        }
        for &def in DEFAULT_TRACKERS {
            let s = def.to_string();
            if !trackers.contains(&s) {
                trackers.push(s);
            }
        }

        let info = root
            .dict_get("info")
            .ok_or_else(|| anyhow!("missing info dict"))?;
        let name = info
            .dict_get("name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("torrent-{}", hex_encode(&info_hash[..4])));
        let piece_length = info
            .dict_get("piece length")
            .and_then(|v| v.as_int())
            .unwrap_or(262_144)
            .max(16_384) as u64;

        let pieces_raw = info
            .dict_get("pieces")
            .and_then(|v| v.as_bytes())
            .unwrap_or(&[]);
        let mut piece_hashes = Vec::with_capacity(pieces_raw.len() / 20);
        for chunk in pieces_raw.chunks_exact(20) {
            let mut h = [0u8; 20];
            h.copy_from_slice(chunk);
            piece_hashes.push(h);
        }

        let total_size = if let Some(len) = info.dict_get("length").and_then(|v| v.as_int()) {
            len.max(0) as u64
        } else if let Some(files) = info.dict_get("files").and_then(|v| v.as_list()) {
            files
                .iter()
                .filter_map(|f| f.dict_get("length").and_then(|v| v.as_int()))
                .map(|n| n.max(0) as u64)
                .sum()
        } else {
            (piece_hashes.len() as u64) * piece_length
        };

        Ok(Self {
            name,
            info_hash,
            total_size,
            piece_length,
            piece_hashes,
            trackers,
        })
    }

    pub fn from_magnet_uri(uri: &str) -> Result<Self> {
        let uri = uri.trim();
        let query = uri
            .strip_prefix("magnet:?")
            .or_else(|| uri.strip_prefix("MAGNET:?"))
            .ok_or_else(|| anyhow!("not a magnet:? URI"))?;

        let mut info_hash: Option<[u8; 20]> = None;
        let mut name: Option<String> = None;
        let mut exact_length: u64 = 0;
        let mut trackers = Vec::new();

        for part in query.split('&') {
            let Some((k, v)) = part.split_once('=') else {
                continue;
            };
            let k_lower = k.to_ascii_lowercase();
            let decoded_v = percent_decode(v);
            match k_lower.as_str() {
                "xt" => {
                    let lower_v = decoded_v.to_ascii_lowercase();
                    if let Some(hash_str) = lower_v.strip_prefix("urn:btih:") {
                        let hash_clean = hash_str.split('&').next().unwrap_or(hash_str);
                        info_hash = hex_decode_20(hash_clean).or_else(|| base32_decode_20(hash_clean));
                    }
                }
                "dn" => {
                    if !decoded_v.trim().is_empty() {
                        name = Some(decoded_v.trim().to_string());
                    }
                }
                "xl" => {
                    if let Ok(len) = decoded_v.trim().parse::<u64>() {
                        exact_length = len;
                    }
                }
                "tr" => {
                    let tr = decoded_v.trim().to_string();
                    if !tr.is_empty() && !trackers.contains(&tr) {
                        trackers.push(tr);
                    }
                }
                _ => {}
            }
        }

        let info_hash = info_hash.ok_or_else(|| anyhow!("magnet URI missing valid urn:btih hash"))?;
        for &def in DEFAULT_TRACKERS {
            let s = def.to_string();
            if !trackers.contains(&s) {
                trackers.push(s);
            }
        }
        let name = name.unwrap_or_else(|| format!("magnet-{}", hex_encode(&info_hash[..6])));

        Ok(Self {
            name,
            info_hash,
            total_size: exact_length,
            piece_length: 262_144,
            piece_hashes: Vec::new(),
            trackers,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct AnnounceResult {
    pub seeders: u32,
    pub leechers: u32,
    pub peers: Vec<SocketAddr>,
}

/// Announces to a UDP tracker (`udp://host:port/announce`) per BEP 15.
pub fn announce_udp_tracker(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    downloaded: u64,
    left: u64,
) -> Result<AnnounceResult> {
    let rest = tracker_url
        .strip_prefix("udp://")
        .ok_or_else(|| anyhow!("not a udp:// tracker"))?;
    let host_port = rest.split('/').next().unwrap_or(rest);
    let addr = host_port
        .to_socket_addrs()?
        .find(|a| a.is_ipv4())
        .ok_or_else(|| anyhow!("could not resolve IPv4 for {host_port}"))?;

    let sock = UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(Duration::from_millis(2200)))?;
    sock.set_write_timeout(Some(Duration::from_millis(2200)))?;
    sock.connect(addr)?;

    // 1. Connect Request
    let tx_id: u32 = 0x39A1_7C25;
    let mut conn_req = [0u8; 16];
    conn_req[0..8].copy_from_slice(&0x0417_2710_1980_u64.to_be_bytes());
    conn_req[8..12].copy_from_slice(&0u32.to_be_bytes()); // action = 0 (connect)
    conn_req[12..16].copy_from_slice(&tx_id.to_be_bytes());
    sock.send(&conn_req)?;

    let mut conn_resp = [0u8; 16];
    let n = sock.recv(&mut conn_resp)?;
    if n < 16 {
        bail!("short UDP tracker connect response");
    }
    let resp_action = u32::from_be_bytes([conn_resp[0], conn_resp[1], conn_resp[2], conn_resp[3]]);
    let resp_tx = u32::from_be_bytes([conn_resp[4], conn_resp[5], conn_resp[6], conn_resp[7]]);
    if resp_action != 0 || resp_tx != tx_id {
        bail!("UDP tracker connect mismatch");
    }
    let connection_id = u64::from_be_bytes([
        conn_resp[8],
        conn_resp[9],
        conn_resp[10],
        conn_resp[11],
        conn_resp[12],
        conn_resp[13],
        conn_resp[14],
        conn_resp[15],
    ]);

    // 2. Announce Request (98 bytes)
    let tx_id2: u32 = 0x52B8_9E11;
    let mut ann_req = [0u8; 98];
    ann_req[0..8].copy_from_slice(&connection_id.to_be_bytes());
    ann_req[8..12].copy_from_slice(&1u32.to_be_bytes()); // action = 1 (announce)
    ann_req[12..16].copy_from_slice(&tx_id2.to_be_bytes());
    ann_req[16..36].copy_from_slice(info_hash);
    ann_req[36..56].copy_from_slice(peer_id);
    ann_req[56..64].copy_from_slice(&downloaded.to_be_bytes());
    ann_req[64..72].copy_from_slice(&left.to_be_bytes());
    ann_req[72..80].copy_from_slice(&0u64.to_be_bytes()); // uploaded
    ann_req[80..84].copy_from_slice(&2u32.to_be_bytes()); // event = 2 (started)
    ann_req[84..88].copy_from_slice(&0u32.to_be_bytes()); // IP = default
    ann_req[88..92].copy_from_slice(&0x1F2E_3D4C_u32.to_be_bytes()); // key
    ann_req[92..96].copy_from_slice(&50i32.to_be_bytes()); // num_want = 50
    ann_req[96..98].copy_from_slice(&6881u16.to_be_bytes()); // port
    sock.send(&ann_req)?;

    let mut buf = [0u8; 1500];
    let n = sock.recv(&mut buf)?;
    if n < 20 {
        bail!("short UDP tracker announce response");
    }
    let action = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let r_tx = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
    if action != 1 || r_tx != tx_id2 {
        bail!("UDP tracker announce action mismatch");
    }
    let leechers = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
    let seeders = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]);
    let mut peers = Vec::new();
    for chunk in buf[20..n].chunks_exact(6) {
        let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
        let port = u16::from_be_bytes([chunk[4], chunk[5]]);
        if port > 0 && !ip.is_unspecified() {
            peers.push(SocketAddr::new(IpAddr::V4(ip), port));
        }
    }
    Ok(AnnounceResult {
        seeders,
        leechers,
        peers,
    })
}

/// Announces to an HTTP/HTTPS tracker using `curl.exe` and parses the Bencoded peer response.
pub fn announce_http_tracker(
    tracker_url: &str,
    info_hash: &[u8; 20],
    peer_id: &[u8; 20],
    downloaded: u64,
    left: u64,
) -> Result<AnnounceResult> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let sep = if tracker_url.contains('?') { '&' } else { '?' };
    let url = format!(
        "{tracker_url}{sep}info_hash={}&peer_id={}&port=6881&uploaded=0&downloaded={downloaded}&left={left}&compact=1&numwant=50",
        url_encode_bytes(info_hash),
        url_encode_bytes(peer_id),
    );

    let mut cmd = std::process::Command::new("curl.exe");
    cmd.args(["-sSL", "--max-time", "5", &url]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output()?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!("HTTP tracker request failed");
    }

    let (val, _) = parse_bencode(&out.stdout)?;
    let seeders = val
        .dict_get("complete")
        .and_then(|v| v.as_int())
        .unwrap_or(0)
        .max(0) as u32;
    let leechers = val
        .dict_get("incomplete")
        .and_then(|v| v.as_int())
        .unwrap_or(0)
        .max(0) as u32;
    let mut peers = Vec::new();
    if let Some(compact) = val.dict_get("peers").and_then(|v| v.as_bytes()) {
        for chunk in compact.chunks_exact(6) {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            if port > 0 && !ip.is_unspecified() {
                peers.push(SocketAddr::new(IpAddr::V4(ip), port));
            }
        }
    }
    Ok(AnnounceResult {
        seeders,
        leechers,
        peers,
    })
}

/// Connects to a single BitTorrent peer and downloads available pieces while respecting
/// `should_stop()` (Pause/Cancel) and `speed_limit_kbps()` (Token-bucket rate limit).
pub fn download_from_peer<FStop, FSpeed, FProgress>(
    peer_addr: SocketAddr,
    meta: &TorrentMeta,
    peer_id: &[u8; 20],
    output_path: &Path,
    completed_pieces: &mut [bool],
    should_stop: FStop,
    speed_limit_kbps: FSpeed,
    mut on_progress: FProgress,
) -> Result<u64>
where
    FStop: Fn() -> bool,
    FSpeed: Fn() -> u32,
    FProgress: FnMut(u64),
{
    let mut stream = TcpStream::connect_timeout(&peer_addr, Duration::from_millis(2500))?;
    stream.set_read_timeout(Some(Duration::from_millis(4000)))?;
    stream.set_write_timeout(Some(Duration::from_millis(4000)))?;

    // 68-byte BitTorrent handshake
    let mut hs = [0u8; 68];
    hs[0] = 19;
    hs[1..20].copy_from_slice(b"BitTorrent protocol");
    hs[25] = 0x10; // BEP 10 extension protocol bit
    hs[28..48].copy_from_slice(&meta.info_hash);
    hs[48..68].copy_from_slice(peer_id);
    stream.write_all(&hs)?;

    let mut peer_hs = [0u8; 68];
    stream.read_exact(&mut peer_hs)?;
    if &peer_hs[1..20] != b"BitTorrent protocol" || &peer_hs[28..48] != &meta.info_hash[..] {
        bail!("invalid BitTorrent peer handshake");
    }

    // Send Interested (length=1, id=2)
    stream.write_all(&[0, 0, 0, 1, 2])?;

    let num_pieces = if !meta.piece_hashes.is_empty() {
        meta.piece_hashes.len()
    } else if meta.total_size > 0 && meta.piece_length > 0 {
        ((meta.total_size + meta.piece_length - 1) / meta.piece_length) as usize
    } else {
        0
    };
    if num_pieces == 0 {
        return Ok(0);
    }

    let mut peer_choking = true;
    let mut bytes_from_peer = 0u64;
    let mut window_start = Instant::now();
    let mut window_bytes = 0u64;

    if let Some(parent) = output_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(output_path)?;

    let mut current_piece: Option<(usize, Vec<u8>, usize, usize)> = None; // (piece_idx, buf, received, next_req_offset)

    while !should_stop() {
        if !peer_choking && current_piece.is_none() {
            if let Some(next_idx) = (0..num_pieces).find(|&i| !completed_pieces.get(i).copied().unwrap_or(true)) {
                let p_len = if next_idx + 1 == num_pieces && meta.total_size > 0 {
                    let rem = meta.total_size % meta.piece_length;
                    if rem == 0 { meta.piece_length } else { rem }
                } else {
                    meta.piece_length
                } as usize;
                current_piece = Some((next_idx, vec![0u8; p_len], 0, 0));
            } else {
                break;
            }
        }

        // Send up to 4 pipelined 16 KB block requests
        if !peer_choking {
            if let Some((p_idx, ref buf, received, ref mut next_off)) = current_piece {
                let p_len = buf.len();
                while *next_off < p_len && (*next_off - received) < 65_536 {
                    let block_len = (p_len - *next_off).min(16_384) as u32;
                    let mut req = [0u8; 17];
                    req[0..4].copy_from_slice(&13u32.to_be_bytes());
                    req[4] = 6; // Request
                    req[5..9].copy_from_slice(&(p_idx as u32).to_be_bytes());
                    req[9..13].copy_from_slice(&(*next_off as u32).to_be_bytes());
                    req[13..17].copy_from_slice(&block_len.to_be_bytes());
                    stream.write_all(&req)?;
                    *next_off += block_len as usize;
                }
            }
        }

        // Read next peer message
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf)?;
        let msg_len = u32::from_be_bytes(len_buf) as usize;
        if msg_len == 0 {
            continue; // keep-alive
        }
        if msg_len > 262_144 {
            bail!("peer message too large: {msg_len}");
        }
        let mut payload = vec![0u8; msg_len];
        stream.read_exact(&mut payload)?;

        match payload[0] {
            0 => peer_choking = true,
            1 => peer_choking = false,
            7 if payload.len() > 9 => {
                let p_idx = u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
                let begin = u32::from_be_bytes([payload[5], payload[6], payload[7], payload[8]]) as usize;
                let block = &payload[9..];

                // Token-bucket speed limiter
                let limit_kbps = speed_limit_kbps();
                if limit_kbps > 0 {
                    window_bytes += block.len() as u64;
                    let target_secs = (window_bytes as f64) / ((limit_kbps as f64) * 1024.0);
                    let elapsed = window_start.elapsed().as_secs_f64();
                    if target_secs > elapsed {
                        let sleep_dur = Duration::from_secs_f64((target_secs - elapsed).min(0.25));
                        std::thread::sleep(sleep_dur);
                    }
                    if window_start.elapsed() >= Duration::from_secs(1) {
                        window_start = Instant::now();
                        window_bytes = 0;
                    }
                }

                let mut piece_done = false;
                if let Some((cur_idx, ref mut buf, ref mut received, _)) = current_piece {
                    if cur_idx == p_idx && begin + block.len() <= buf.len() {
                        buf[begin..begin + block.len()].copy_from_slice(block);
                        *received += block.len();
                        bytes_from_peer += block.len() as u64;
                        on_progress(block.len() as u64);
                        if *received >= buf.len() {
                            piece_done = true;
                        }
                    }
                }

                if piece_done {
                    if let Some((cur_idx, buf, _, _)) = current_piece.take() {
                        let hash_ok = meta
                            .piece_hashes
                            .get(cur_idx)
                            .map_or(true, |expected| &sha1_digest(&buf) == expected);
                        if hash_ok {
                            let offset = (cur_idx as u64) * meta.piece_length;
                            file.seek(SeekFrom::Start(offset))?;
                            file.write_all(&buf)?;
                            if let Some(slot) = completed_pieces.get_mut(cur_idx) {
                                *slot = true;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    Ok(bytes_from_peer)
}

pub fn resolve_torrent_target_path(save_dir: &Path, name: &str) -> PathBuf {
    let clean: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect();
    save_dir.join(if clean.trim().is_empty() {
        "torrent-download.bin"
    } else {
        clean.trim()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_nist_vectors() {
        assert_eq!(
            hex_encode(&sha1_digest(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex_encode(&sha1_digest(b"")),
            "da39a3ee5e6b4b0d3255bfef95601890afd80709"
        );
    }

    #[test]
    fn parses_magnet_uri_hex_and_base32() {
        let magnet = "magnet:?xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d&dn=Ubuntu+24.04+Desktop.iso&xl=4950000000&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337%2Fannounce";
        let meta = TorrentMeta::from_magnet_uri(magnet).expect("magnet parse");
        assert_eq!(meta.name, "Ubuntu 24.04 Desktop.iso");
        assert_eq!(meta.total_size, 4_950_000_000);
        assert_eq!(
            meta.info_hash_hex(),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert!(!meta.trackers.is_empty());

        // 32-char Base32 magnet hash
        let b32_magnet = "magnet:?xt=urn:btih:VGMT4NSDQ2AVRORTE5Y6QUDCOZONTWE5&dn=Sample";
        let meta_b32 = TorrentMeta::from_magnet_uri(b32_magnet).expect("base32 magnet");
        assert_eq!(meta_b32.name, "Sample");
    }

    #[test]
    fn parses_bencoded_torrent_metadata() {
        let piece_hash = [0x42u8; 20];
        let mut torrent_bytes = Vec::new();
        torrent_bytes.extend_from_slice(
            b"d8:announce40:http://tracker.example.com:8080/announce4:infod6:lengthi1048576e4:name10:sample.iso12:piece lengthi262144e6:pieces20:",
        );
        torrent_bytes.extend_from_slice(&piece_hash);
        torrent_bytes.extend_from_slice(b"ee");

        let meta = TorrentMeta::from_torrent_bytes(&torrent_bytes).expect("parse .torrent");
        assert_eq!(meta.name, "sample.iso");
        assert_eq!(meta.total_size, 1_048_576);
        assert_eq!(meta.piece_length, 262_144);
        assert_eq!(meta.piece_hashes, vec![piece_hash]);
        assert!(meta
            .trackers
            .contains(&"http://tracker.example.com:8080/announce".to_string()));
    }
}
