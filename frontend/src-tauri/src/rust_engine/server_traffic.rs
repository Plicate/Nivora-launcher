// Наблюдаемые счётчики TCP-полезной нагрузки сокетов одного дерева процессов сервера.
//
// Linux: владение из /proc + INET_DIAG/TCP_INFO — без root, без снифинга, без
// изменений firewall и без общесистемных счётчиков интерфейсов.
//
// Windows: GetExtendedTcpTable (OWNER_PID) + TCP_ESTATS_DATA через iphlpapi —
// байты точны на соединение, без прав администратора. Сбор включается на каждое
// соединение отдельно, поэтому первая выборка нового соединения может быть нулём.
//
// Короткоживущие соединения между выборками могут быть пропущены на обеих
// платформах: итоги — это наблюдённое, а не биллинг.
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

fn u16_be(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("два байта"))
}

fn u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("два байта"))
}

fn u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("четыре байта"))
}

fn u64_le(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("восемь байт"))
}

pub(crate) fn socket_inodes(pid: u32) -> Result<HashSet<u32>, String> {
    socket_inodes_from(pid, Path::new("/proc"))
}

fn socket_inodes_from(pid: u32, proc_root: &Path) -> Result<HashSet<u32>, String> {
    let mut pending = vec![pid];
    let mut seen: HashSet<u32> = HashSet::new();
    let mut result: HashSet<u32> = HashSet::new();
    let mut root_read = false;
    let mut denied = false;
    while let Some(current) = pending.pop() {
        if seen.len() >= 1024 {
            break;
        }
        if current == 0 || seen.contains(&current) {
            continue;
        }
        seen.insert(current);
        let root = proc_root.join(current.to_string());
        match fs::read_dir(root.join("fd")) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if let Ok(target) = fs::read_link(entry.path()) {
                        let target = target.to_string_lossy();
                        if let Some(inner) = target.strip_prefix("socket:[").and_then(|rest| rest.strip_suffix(']')) {
                            if let Ok(inode) = inner.parse::<u32>() {
                                result.insert(inode);
                            }
                        }
                    }
                }
                if current == pid {
                    root_read = true;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => denied = true,
            Err(_) => {}
        }
        if let Ok(children) = fs::read_to_string(root.join("task").join(current.to_string()).join("children")) {
            pending.extend(children.split_whitespace().filter_map(|value| value.parse::<u32>().ok()));
        }
    }
    if !root_read {
        return Err("Нет доступа к socket FD серверного процесса".to_owned());
    }
    if denied {
        return Err("Нет доступа к сокетам части дерева серверного процесса".to_owned());
    }
    Ok(result)
}

/// Разбор одного inet_diag-сообщения: интересует сокет с нужным inode и портом.
fn parse_diag_message(raw: &[u8], inodes: &HashSet<u32>, port: u16) -> Result<Option<(String, u64, u64)>, String> {
    if raw.len() < 72 {
        return Ok(None);
    }
    let inode = u32_le(raw, 68);
    let source_port = u16_be(raw, 4);
    // raw[1] == 10 — TCP_LISTEN: слушающий сокет не считается.
    if !inodes.contains(&inode) || source_port != port || raw[1] == 10 {
        return Ok(None);
    }
    let mut offset = 72;
    while offset + 4 <= raw.len() {
        let length = u16_le(raw, offset) as usize;
        let kind = u16_le(raw, offset + 2);
        if length < 4 || offset + length > raw.len() {
            return Err("Некорректный ответ INET_DIAG".to_owned());
        }
        if kind == 2 {
            // INET_DIAG_INFO: стабильный ABI tcp_info ядра.
            let info = &raw[offset + 4..offset + length];
            if info.len() < 208 {
                return Err("Ядро не предоставляет TCP_INFO bytes_sent/bytes_received".to_owned());
            }
            let received = u64_le(info, 128);
            let sent = u64_le(info, 200);
            // Cookie переживает переиспользование inode и разделяет IPv4/IPv6.
            let cookie: String = raw[44..52].iter().map(|byte| format!("{byte:02x}")).collect();
            let key = format!("{}:{inode}:{cookie}", raw[0]);
            return Ok(Some((key, received, sent)));
        }
        offset += (length + 3) & !3;
    }
    Err("TCP_INFO для сокета сервера недоступен".to_owned())
}

pub(crate) fn tcp_counters(pid: u32, port: u16) -> Result<HashMap<String, (u64, u64)>, String> {
    #[cfg(windows)]
    {
        windows_tcp_counters(pid, port)
    }
    #[cfg(not(windows))]
    {
        linux_tcp_counters(pid, port)
    }
}

fn traffic_source() -> &'static str {
    if cfg!(windows) {
        "windows-tcp-estats"
    } else {
        "linux-inet-diag-tcp-info"
    }
}

#[cfg(not(windows))]
fn linux_tcp_counters(pid: u32, port: u16) -> Result<HashMap<String, (u64, u64)>, String> {
    let inodes = socket_inodes(pid)?;
    if inodes.is_empty() {
        return Ok(HashMap::new());
    }
    let mut result: HashMap<String, (u64, u64)> = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs_f64(0.7);
    let mut budget: i64 = 4 * 1024 * 1024;
    let mut sequence = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.subsec_nanos() as u32)
        .unwrap_or(0)) as u32;

    let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_INET_DIAG) };
    if fd < 0 {
        return Err("Метрики TCP серверных сокетов доступны на Linux и Windows".to_owned());
    }
    let outcome = (|| -> Result<(), String> {
        let mut local: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        local.nl_family = libc::AF_NETLINK as u16;
        if unsafe { libc::bind(fd, &local as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_nl>() as u32) } != 0 {
            return Err("Метрики TCP серверных сокетов доступны на Linux и Windows".to_owned());
        }
        for family in [libc::AF_INET, libc::AF_INET6] {
            sequence = sequence.wrapping_add(1);
            // inet_diag_req_v2: family, proto, ext=2, pad, states=0xffffffff, sockid(48).
            let mut request = Vec::with_capacity(56);
            request.extend_from_slice(&[family as u8, libc::IPPROTO_TCP as u8, 2, 0]);
            request.extend_from_slice(&0xffffffffu32.to_le_bytes());
            request.extend_from_slice(&[0u8; 48]);
            let mut message = Vec::with_capacity(16 + request.len());
            message.extend_from_slice(((16 + request.len()) as u32).to_le_bytes());
            message.extend_from_slice(&20u16.to_le_bytes()); // SOCK_DIAG_BY_FAMILY
            message.extend_from_slice(&0x301u16.to_le_bytes()); // REQUEST | DUMP
            message.extend_from_slice(&sequence.to_le_bytes());
            message.extend_from_slice(&0u32.to_le_bytes());
            message.extend_from_slice(&request);
            let mut kernel: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
            kernel.nl_family = libc::AF_NETLINK as u16;
            let sent = unsafe {
                libc::sendto(
                    fd,
                    message.as_ptr().cast(),
                    message.len(),
                    0,
                    &kernel as *const _ as *const libc::sockaddr,
                    std::mem::size_of::<libc::sockaddr_nl>() as u32,
                )
            };
            if sent < 0 {
                return Err("Метрики TCP серверных сокетов доступны на Linux и Windows".to_owned());
            }

            let mut done = false;
            while !done {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() || budget <= 0 {
                    return Err("Превышен лимит чтения TCP-метрик".to_owned());
                }
                let timeout = libc::timeval {
                    tv_sec: remaining.as_secs() as libc::time_t,
                    tv_usec: remaining.subsec_micros() as libc::suseconds_t,
                };
                unsafe { libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_RCVTIMEO, &timeout as *const _ as *const libc::c_void, std::mem::size_of::<libc::timeval>() as u32) };
                let mut buffer = vec![0u8; 65536];
                let mut name: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
                let mut iov = libc::iovec {
                    iov_base: buffer.as_mut_ptr().cast(),
                    iov_len: buffer.len(),
                };
                let mut message_header: libc::msghdr = unsafe { std::mem::zeroed() };
                message_header.msg_name = &mut name as *mut _ as *mut libc::c_void;
                message_header.msg_namelen = std::mem::size_of::<libc::sockaddr_nl>() as u32;
                message_header.msg_iov = &mut iov;
                message_header.msg_iovlen = 1;
                let received = unsafe { libc::recvmsg(fd, &mut message_header, 0) };
                if received < 0 {
                    return Err("Превышен лимит чтения TCP-метрик".to_owned());
                }
                let count = received as usize;
                budget -= count as i64;
                let truncated = message_header.msg_flags & libc::MSG_TRUNC as i32 != 0;
                if count == 0 || truncated || name.nl_pid != 0 {
                    return Err("Неполный ответ ядра TCP-метрик".to_owned());
                }
                let data = &buffer[..count];
                let mut offset = 0usize;
                while offset + 16 <= data.len() {
                    let length = u32_le(data, offset) as usize;
                    let kind = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().expect("два байта"));
                    let message_flags = u16::from_le_bytes(data[offset + 6..offset + 8].try_into().expect("два байта"));
                    let message_seq = u32_le(data, offset + 8);
                    if length < 16 || offset + length > data.len() {
                        return Err("Неполный netlink-пакет".to_owned());
                    }
                    let raw = &data[offset + 16..offset + length];
                    offset += (length + 3) & !3;
                    if message_seq != sequence {
                        continue;
                    }
                    if message_flags & 0x10 != 0 {
                        // NLM_F_DUMP_INTR
                        return Err("TCP-таблица изменилась во время чтения".to_owned());
                    }
                    if kind == 3 {
                        // NLMSG_DONE: ненулевой знаковый код — отказ ядра.
                        if raw.len() >= 4 && i32::from_le_bytes(raw[0..4].try_into().expect("четыре байта")) != 0 {
                            return Err("Ядро прервало чтение TCP-таблицы".to_owned());
                        }
                        done = true;
                        break;
                    }
                    if kind == 2 {
                        // NLMSG_ERROR: код — знаковый int в первых четырёх байтах.
                        let code = if raw.len() >= 4 { u32_le(raw, 0) as i32 } else { -1 };
                        if code != 0 {
                            return Err(format!("INET_DIAG недоступен (errno {})", code.abs()));
                        }
                    } else if kind == 20 {
                        if let Some((key, received, sent)) = parse_diag_message(raw, &inodes, port)? {
                            result.insert(key, (received, sent));
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    unsafe { libc::close(fd) };
    outcome.map(|_| result)
}

pub(crate) struct ServerTrafficTracker {
    pid: u32,
    port: u16,
    previous: HashMap<String, (u64, u64)>,
    last_at: Option<Instant>,
    received: u64,
    sent: u64,
    cached: Value,
    next_sample: Instant,
}

impl ServerTrafficTracker {
    pub(crate) fn new(pid: u32, port: u16) -> Self {
        let mut cached = Self::unavailable_json(0, 0, "Ожидание первой TCP-выборки");
        cached["network_rx_bps"] = Value::Null;
        cached["network_tx_bps"] = Value::Null;
        Self {
            pid,
            port,
            previous: HashMap::new(),
            last_at: None,
            received: 0,
            sent: 0,
            cached,
            next_sample: Instant::now(),
        }
    }

    fn unavailable_json(received: u64, sent: u64, reason: &str) -> Value {
        json!({
            "network_available": false,
            "network_source": traffic_source(),
            "network_scope": "observed-game-tcp",
            "network_error": reason,
            "network_rx_bps": null,
            "network_tx_bps": null,
            "network_rx_bytes": received,
            "network_tx_bytes": sent,
        })
    }

    pub(crate) fn sample(&mut self, running: bool) -> Value {
        let now = Instant::now();
        if !running {
            let mut value = self.cached.clone();
            if value["network_available"].as_bool().unwrap_or(false) {
                value["network_rx_bps"] = json!(0.0);
                value["network_tx_bps"] = json!(0.0);
            } else {
                value["network_rx_bps"] = Value::Null;
                value["network_tx_bps"] = Value::Null;
            }
            return value;
        }
        if now < self.next_sample {
            return self.cached.clone();
        }
        self.next_sample = now + Duration::from_secs_f64(0.8);
        match tcp_counters(self.pid, self.port) {
            Ok(current) => {
                let mut rx = 0u64;
                let mut tx = 0u64;
                for (key, (received, sent)) in &current {
                    let (previous_rx, previous_tx) = self.previous.get(key).copied().unwrap_or((0, 0));
                    rx += received.saturating_sub(previous_rx);
                    tx += sent.saturating_sub(previous_tx);
                }
                self.received += rx;
                self.sent += tx;
                let elapsed = self.last_at.map(|last_at| (now - last_at).as_secs_f64().max(0.001));
                let round2 = |value: f64| (value * 100.0).round() / 100.0;
                self.cached = json!({
                    "network_available": true,
                    "network_source": traffic_source(),
                    "network_scope": "observed-game-tcp",
                    "network_error": "",
                    "network_rx_bps": elapsed.map(|seconds| round2(rx as f64 / seconds)),
                    "network_tx_bps": elapsed.map(|seconds| round2(tx as f64 / seconds)),
                    "network_rx_bytes": self.received,
                    "network_tx_bytes": self.sent,
                });
                self.previous = current;
                self.last_at = now;
            }
            Err(message) => {
                self.cached = Self::unavailable_json(self.received, self.sent, &message);
                self.next_sample = now + Duration::from_secs_f64(5.0);
            }
        }
        self.cached.clone()
    }
}

// ── Windows: счётчики на соединение через iphlpapi ─────────────────────────
//
// GetExtendedTcpTable отдаёт соединения с PID владельца, GetPerTcpConnectionEstats
// с TcpConnectionEstatsData — накопленные DataBytesIn/Out. Оба вызова
// документированы и не требуют прав администратора. Порядок аргументов
// GetPerTcpConnectionEstats повторяет исходный Python-код позиционно.

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetPerTcpConnectionEstats, SetPerTcpConnectionEstats, MIB_TCPROW_OWNER_PID,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    const TCP_ESTATS_DATA: u32 = 1;
    const MIB_TCP_STATE_ESTAB: u32 = 5;
    const TCP_TABLE_OWNER_PID_ALL: u32 = 5;

    pub(super) fn windows_pid_tree(pid: u32) -> Result<HashSet<u32>, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err("CreateToolhelp32Snapshot failed".to_owned());
        }
        let mut parents: HashMap<u32, u32> = HashMap::new();
        unsafe {
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
        }
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (child, parent) in &parents {
            children.entry(*parent).or_default().push(*child);
        }
        let mut tree: HashSet<u32> = HashSet::new();
        let mut pending = vec![pid];
        while let Some(current) = pending.pop() {
            if current == 0 || tree.contains(&current) {
                continue;
            }
            tree.insert(current);
            if let Some(list) = children.get(&current) {
                pending.extend(list.iter().copied());
            }
        }
        Ok(tree)
    }

    pub(super) struct OwnedRow {
        pub row: MIB_TCPROW_OWNER_PID,
        pub key: String,
    }

    /// Строки сервера в состоянии ESTAB из MIB_TCPTABLE_OWNER_PID.
    pub(super) fn parse_owner_pid_table(raw: &[u8], pids: &HashSet<u32>, port: u16) -> Result<Vec<OwnedRow>, String> {
        if raw.len() < 4 {
            return Err("Некорректная TCP-таблица Windows".to_owned());
        }
        let count = u32_le(raw, 0) as usize;
        let mut rows = Vec::new();
        for index in 0..count.min(65536) {
            let offset = 4 + index * 24;
            if offset + 24 > raw.len() {
                return Err("Обрезанная TCP-таблица Windows".to_owned());
            }
            let state = u32_le(raw, offset);
            let local_addr = u32_le(raw, offset + 4);
            let local_port = u32_le(raw, offset + 8);
            let remote_addr = u32_le(raw, offset + 12);
            let remote_port = u32_le(raw, offset + 16);
            let owner = u32_le(raw, offset + 20);
            if !pids.contains(&owner) || state != MIB_TCP_STATE_ESTAB {
                continue;
            }
            let local_native = u16::from_be((local_port & 0xFFFF) as u16);
            if local_native != port {
                continue;
            }
            let remote_native = u16::from_be((remote_port & 0xFFFF) as u16);
            let local_ip = ipv4_dotted(local_addr);
            let remote_ip = ipv4_dotted(remote_addr);
            let key = format!("{owner}:{local_ip}:{local_native}>{remote_ip}:{remote_native}");
            rows.push(OwnedRow {
                row: MIB_TCPROW_OWNER_PID {
                    dwState: state,
                    dwLocalAddr: local_addr,
                    dwLocalPort: local_port,
                    dwRemoteAddr: remote_addr,
                    dwRemotePort: remote_port,
                    dwOwningPid: owner,
                },
                key,
            });
        }
        Ok(rows)
    }

    pub(super) fn ipv4_dotted(address: u32) -> String {
        let bytes = address.to_le_bytes();
        format!("{}.{}.{}.{}", bytes[0], bytes[1], bytes[2], bytes[3])
    }

    pub(super) fn windows_tcp_counters(pid: u32, port: u16) -> Result<HashMap<String, (u64, u64)>, String> {
        use std::sync::Mutex;
        // Сбор включается на соединение; список нужен, чтобы не включать повторно.
        static ESTATS_ENABLED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

        let pids = windows_pid_tree(pid)?;
        let mut size: u32 = 0;
        unsafe {
            GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, 2, TCP_TABLE_OWNER_PID_ALL, 0);
        }
        let mut buffer = vec![0u8; size as usize];
        let result = unsafe {
            GetExtendedTcpTable(buffer.as_mut_ptr().cast(), &mut size, 0, 2, TCP_TABLE_OWNER_PID_ALL, 0)
        };
        if result != 0 {
            return Err(format!("GetExtendedTcpTable failed ({result})"));
        }
        let rows = parse_owner_pid_table(&buffer, &pids, port)?;

        let mut counters: HashMap<String, (u64, u64)> = HashMap::new();
        let mut enabled = ESTATS_ENABLED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let enabled = enabled.get_or_insert_with(HashSet::new);
        for owned in &rows {
            if !enabled.contains(&owned.key) {
                // Счётчики соединения копятся только после явного включения сбора.
                let mut rw = [1u8]; // TCP_ESTATS_DATA_RW_V0 { EnableCollection: TRUE }
                unsafe {
                    SetPerTcpConnectionEstats(&owned.row, TCP_ESTATS_DATA, rw.as_mut_ptr().cast(), rw.len() as u32);
                }
                enabled.insert(owned.key.clone());
            }
            let mut rod = [0u8; 16]; // TCP_ESTATS_DATA_ROD_V0 { DataBytesIn, DataBytesOut }
            let status = unsafe {
                GetPerTcpConnectionEstats(
                    &owned.row,
                    TCP_ESTATS_DATA,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    rod.as_mut_ptr().cast(),
                    rod.len() as u32,
                )
            };
            if status == 0 {
                counters.insert(owned.key.clone(), (u64_le(&rod, 0), u64_le(&rod, 8)));
            }
        }
        // Ушедшие соединения больше не требуют включённого сбора.
        if enabled.len() > 4096 {
            enabled.retain(|key| counters.contains_key(key));
        }
        Ok(counters)
    }
}

#[cfg(windows)]
use windows_impl::windows_tcp_counters;

#[cfg(test)]
mod tests {
    use super::*;

    fn diag_message(inode: u32, port: u16, state: u8, family: u8) -> Vec<u8> {
        let mut raw = vec![0u8; 72];
        raw[0] = family;
        raw[1] = state;
        raw[4..6].copy_from_slice(&port.to_be_bytes());
        for (index, byte) in [0xdeu8, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04].iter().enumerate() {
            raw[44 + index] = *byte;
        }
        raw[68..72].copy_from_slice(&inode.to_le_bytes());
        // Атрибут INET_DIAG_INFO с телом tcp_info на 208 байт.
        let mut info = vec![0u8; 208];
        info[128..136].copy_from_slice(&1234u64.to_le_bytes());
        info[200..208].copy_from_slice(&5678u64.to_le_bytes());
        let attribute_length = (4 + info.len()) as u16;
        raw.extend_from_slice(&attribute_length.to_le_bytes());
        raw.extend_from_slice(&2u16.to_le_bytes());
        raw.extend_from_slice(&info);
        raw
    }

    #[test]
    fn diag_message_extracts_counters_and_key() {
        let mut inodes = HashSet::new();
        inodes.insert(4242);
        let raw = diag_message(4242, 25565, 1, 2);
        let (key, received, sent) = parse_diag_message(&raw, &inodes, 25565).unwrap().unwrap();
        assert_eq!((received, sent), (1234, 5678));
        assert_eq!(key, format!("2:4242:deadbeef01020304"));
    }

    #[test]
    fn diag_message_ignores_foreign_sockets() {
        let mut inodes = HashSet::new();
        inodes.insert(4242);
        // Чужой inode.
        assert!(parse_diag_message(&diag_message(999, 25565, 1, 2), &inodes, 25565).unwrap().is_none());
        // Чужой порт.
        assert!(parse_diag_message(&diag_message(4242, 25566, 1, 2), &inodes, 25565).unwrap().is_none());
        // Слушающий сокет (TCP_LISTEN = 10).
        assert!(parse_diag_message(&diag_message(4242, 25565, 10, 2), &inodes, 25565).unwrap().is_none());
        // Короткий пакет.
        assert!(parse_diag_message(&[0u8; 72], &inodes, 25565).unwrap().is_none());
    }

    #[test]
    fn diag_message_rejects_broken_attributes() {
        let mut inodes = HashSet::new();
        inodes.insert(4242);
        // Атрибут с длиной меньше заголовка.
        let mut raw = diag_message(4242, 25565, 1, 2);
        raw[72..74].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(
            parse_diag_message(&raw, &inodes, 25565).unwrap_err(),
            "Некорректный ответ INET_DIAG"
        );
        // Есть только атрибуты другого типа — TCP_INFO не найден.
        let mut raw = diag_message(4242, 25565, 1, 2);
        raw[74..76].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(
            parse_diag_message(&raw, &inodes, 25565).unwrap_err(),
            "TCP_INFO для сокета сервера недоступен"
        );
    }

    #[test]
    fn tracker_reports_waiting_state_before_first_sample() {
        let mut tracker = ServerTrafficTracker::new(999999, 25565);
        let first = tracker.sample(false);
        assert_eq!(first["network_available"], json!(false));
        assert_eq!(first["network_error"], json!("Ожидание первой TCP-выборки"));
        assert_eq!(first["network_rx_bps"], json!(null));
        // Недоступность переводит повторную попытку в 5-секундный интервал.
        let failed = tracker.sample(true);
        assert_eq!(failed["network_available"], json!(false));
        assert_eq!(
            failed["network_error"],
            json!("Нет доступа к socket FD серверного процесса")
        );
        // До истечения интервала возвращается кэш.
        assert_eq!(tracker.sample(true)["network_error"], json!("Нет доступа к socket FD серверного процесса"));
    }

    #[test]
    fn windows_row_table_parsing_matches_layout() {
        // Таблица из двух строк: наша в ESTAB с нужным портом, чужая мимо.
        let pids: HashSet<u32> = [7u32].into_iter().collect();
        let mut table = Vec::new();
        table.extend_from_slice(&2u32.to_le_bytes());
        let mut push_row = |table: &mut Vec<u8>, state: u32, lport: u32, owner: u32| {
            table.extend_from_slice(&state.to_le_bytes());
            table.extend_from_slice(&0x0100_0A0Au32.to_le_bytes());
            table.extend_from_slice(&(lport.to_be() as u32).to_le_bytes()); // порт в сетевом порядке
            table.extend_from_slice(&0x0808_0808u32.to_le_bytes());
            table.extend_from_slice(&0u32.to_le_bytes());
            table.extend_from_slice(&owner.to_le_bytes());
        };
        push_row(&mut table, 5, 25565, 7);
        push_row(&mut table, 5, 25565, 8);
        let rows = windows_table_rows_for_test(&table, &pids, 25565);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], "7:10.10.0.1:25565>8.8.8.8:0");
    }

    #[cfg(windows)]
    fn windows_table_rows_for_test(raw: &[u8], pids: &HashSet<u32>, port: u16) -> Vec<String> {
        windows_impl::parse_owner_pid_table(raw, pids, port)
            .unwrap()
            .into_iter()
            .map(|owned| owned.key)
            .collect()
    }

    #[cfg(not(windows))]
    fn windows_table_rows_for_test(raw: &[u8], pids: &HashSet<u32>, port: u16) -> Vec<String> {
        // Разбор таблицы — чистая функция; повторяем логику без iphlpapi.
        let count = u32_le(raw, 0) as usize;
        let mut rows = Vec::new();
        for index in 0..count.min(65536) {
            let offset = 4 + index * 24;
            if offset + 24 > raw.len() {
                break;
            }
            let state = u32_le(raw, offset);
            let local_addr = u32_le(raw, offset + 4);
            let local_port = u32_le(raw, offset + 8);
            let remote_addr = u32_le(raw, offset + 12);
            let remote_port = u32_le(raw, offset + 16);
            let owner = u32_le(raw, offset + 20);
            if !pids.contains(&owner) || state != 5 {
                continue;
            }
            let local_native = u16::from_be((local_port & 0xFFFF) as u16);
            if local_native != port {
                continue;
            }
            let remote_native = u16::from_be((remote_port & 0xFFFF) as u16);
            let local_bytes = local_addr.to_le_bytes();
            let remote_bytes = remote_addr.to_le_bytes();
            rows.push(format!(
                "{owner}:{}.{}.{}.{}:{local_native}>{}.{}.{}.{}:{remote_native}",
                local_bytes[0], local_bytes[1], local_bytes[2], local_bytes[3],
                remote_bytes[0], remote_bytes[1], remote_bytes[2], remote_bytes[3]
            ));
        }
        rows
    }
}
