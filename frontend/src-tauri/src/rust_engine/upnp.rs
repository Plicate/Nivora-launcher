// ограниченный UPnP IGD-клиент. Трафик к роутеру никогда не идёт через общий
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use url::Url;

pub(crate) const MAX_XML_BYTES: usize = 256 * 1024;
const SOAP_NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const SERVICE_TYPES: &[&str] = &[
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];
const SSDP_MULTICAST: &str = "239.255.255.250:1900";

#[derive(Debug, Clone)]
pub(crate) struct UpnpError {
    pub message: String,
    pub code: String,
}

impl UpnpError {
    fn new(message: &str) -> Self {
        Self { message: message.to_owned(), code: String::new() }
    }

    fn with_code(message: &str, code: &str) -> Self {
        Self { message: message.to_owned(), code: code.to_owned() }
    }
}

type UpnpResult<T> = Result<T, UpnpError>;

/// строгий разбор dotted-quad: как IPv4Address — без ведущих нулей и мусора.
pub(crate) fn parse_ipv4(value: &str) -> Option<Ipv4Addr> {
    let octets: Vec<&str> = value.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (index, part) in octets.iter().enumerate() {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        bytes[index] = part.parse::<u8>().ok()?;
    }
    Some(Ipv4Addr::from(bytes))
}

pub(crate) fn lan_ipv4(value: &str) -> bool {
    let Some(address) = parse_ipv4(value) else { return false };
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
}

pub(crate) fn router_url(url: &str, peer: &str) -> UpnpResult<(String, u16, String)> {
    if url.len() > 2048 || url.chars().any(|character| (character as u32) < 32) {
        return Err(UpnpError::new("UPnP может обращаться только к локальному роутеру, ответившему на SSDP"));
    }
    let parsed = Url::parse(url).map_err(|_| UpnpError::new("Некорректный адрес UPnP-роутера"))?;
    let port = parsed.port_or_known_default().unwrap_or(80);
    let host = parsed.host_str().unwrap_or("");
    let has_credentials = !parsed.username().is_empty() || parsed.password().is_some();
    if parsed.scheme() != "http"
        || !lan_ipv4(peer)
        || host != peer
        || has_credentials
        || parsed.fragment().is_some()
        || !(1..=65535).contains(&port)
    {
        return Err(UpnpError::new("UPnP может обращаться только к локальному роутеру, ответившему на SSDP"));
    }
    let mut path = parsed.path().to_owned();
    if path.is_empty() {
        path.push('/');
    }
    if let Some(query) = parsed.query() {
        path.push('?');
        path.push_str(query);
    }
    Ok((host.to_owned(), port, path))
}

struct RawResponse {
    status: u16,
    body: Vec<u8>,
}

fn parse_http_response(raw: &[u8]) -> Option<RawResponse> {
    let separator = find_subslice(raw, b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..separator]).ok()?;
    let body_raw = &raw[separator + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next()?;
    let status: u16 = status_line.split(' ').nth(1)?.parse().ok()?;

    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "content-length" {
            content_length = value.parse().ok();
        } else if name == "transfer-encoding" && value.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        }
    }

    let body = if chunked {
        decode_chunked(body_raw)?
    } else if let Some(length) = content_length {
        body_raw.get(..length.min(body_raw.len()))?.to_vec()
    } else {
        body_raw.to_vec()
    };
    Some(RawResponse { status, body })
}

fn decode_chunked(raw: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut offset = 0usize;
    loop {
        let line_end = find_subslice(&raw[offset..], b"\r\n")? + offset;
        let size_text = std::str::from_utf8(&raw[offset..line_end]).ok()?;
        let size_text = size_text.split(';').next()?.trim();
        let size = usize::from_str_radix(size_text, 16).ok()?;
        offset = line_end + 2;
        if size == 0 {
            return Some(body);
        }
        let chunk = raw.get(offset..offset + size)?;
        body.extend_from_slice(chunk);
        offset += size + 2; // данные и завершающий CRLF
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// прямой запрос к роутеру: никакой переадресации, лимит тела MAX_XML_BYTES.
pub(crate) fn router_request(
    url: &str,
    peer: &str,
    body: Option<&[u8]>,
    headers: &[(&str, &str)],
) -> UpnpResult<(u16, Vec<u8>)> {
    let (host, port, path) = router_url(url, peer)?;
    let address = SocketAddr::V4(SocketAddrV4::new(parse_ipv4(&host).ok_or_else(|| UpnpError::new("Некорректный адрес UPnP-роутера"))?, port));
    let timeout = Duration::from_secs(3);
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .and_then(|stream| {
            stream.set_read_timeout(Some(timeout))?;
            stream.set_write_timeout(Some(timeout))?;
            Ok(stream)
        })
        .map_err(|_| UpnpError::new("Не удалось связаться с UPnP-роутером"))?;

    let method = if body.is_some() { "POST" } else { "GET" };
    let mut request = format!("{method} {path} HTTP/1.1\r\n");
    if port == 80 {
        request.push_str(&format!("Host: {host}\r\n"));
    } else {
        request.push_str(&format!("Host: {host}:{port}\r\n"));
    }
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .and_then(|_| match body {
            Some(body) => stream.write_all(body),
            None => Ok(()),
        })
        .map_err(|_| UpnpError::new("Не удалось связаться с UPnP-роутером"))?;

    let mut raw = Vec::new();
    let mut limited = stream.take((MAX_XML_BYTES + 4096) as u64);
    limited
        .read_to_end(&mut raw)
        .map_err(|_| UpnpError::new("Не удалось связаться с UPnP-роутером"))?;
    let response = parse_http_response(&raw).ok_or_else(|| UpnpError::new("Не удалось связаться с UPnP-роутером"))?;
    if (300..400).contains(&response.status) {
        return Err(UpnpError::new("UPnP-роутер вернул запрещённое перенаправление"));
    }
    if response.body.len() > MAX_XML_BYTES {
        return Err(UpnpError::new("Ответ UPnP-роутера слишком большой"));
    }
    Ok((response.status, response.body))
}

pub(crate) struct XmlNode {
    pub tag: String,
    pub text: String,
    pub children: Vec<XmlNode>,
}

impl XmlNode {
    /// все узлы поддерева в порядке документа, начиная с себя (как Element.iter).
    fn iter_nodes<'a>(&'a self, sink: &mut Vec<&'a XmlNode>) {
        sink.push(self);
        for child in &self.children {
            child.iter_nodes(sink);
        }
    }

    pub(crate) fn find_all<'a>(&'a self, name: &str) -> Vec<&'a XmlNode> {
        let mut nodes = Vec::new();
        self.iter_nodes(&mut nodes);
        nodes.into_iter().filter(|node| node.tag == name).collect()
    }

    pub(crate) fn first_text(&self, name: &str) -> Option<String> {
        let mut nodes = Vec::new();
        self.iter_nodes(&mut nodes);
        nodes
            .into_iter()
            .find(|node| node.tag == name)
            .map(|node| node.text.trim().to_owned())
    }
}

pub(crate) fn child_text(element: &XmlNode, name: &str) -> String {
    element
        .children
        .iter()
        .find(|child| child.tag == name)
        .map(|child| child.text.trim().to_owned())
        .unwrap_or_default()
}

pub(crate) fn parse_xml(raw: &[u8]) -> UpnpResult<XmlNode> {
    if raw.len() > MAX_XML_BYTES {
        return Err(UpnpError::new("Ответ UPnP-роутера слишком большой"));
    }
    let text = std::str::from_utf8(raw).map_err(|_| UpnpError::new("Роутер вернул некорректный или небезопасный XML"))?;
    let mut parser = XmlParser { bytes: text.as_bytes(), position: 0 };
    let root = parser.parse_root().map_err(|_| UpnpError::new("Роутер вернул некорректный или небезопасный XML"))?;
    Ok(root)
}

struct XmlParser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> XmlParser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.position).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.position += 1;
        }
    }

    fn parse_root(&mut self) -> Result<XmlNode, ()> {
        self.skip_prolog()?;
        let root = self.parse_element(0)?;
        self.skip_whitespace();
        if self.position != self.bytes.len() {
            return Err(());
        }
        Ok(root)
    }

    fn skip_prolog(&mut self) -> Result<(), ()> {
        loop {
            self.skip_whitespace();
            if self.starts_with(b"<?") {
                self.skip_until(b"?>")?;
            } else if self.starts_with(b"<!--") {
                self.skip_until(b"-->")?;
            } else {
                return Ok(());
            }
        }
    }

    fn starts_with(&self, prefix: &[u8]) -> bool {
        self.bytes[self.position..].starts_with(prefix)
    }

    fn skip_until(&mut self, marker: &[u8]) -> Result<(), ()> {
        let start = self.position + marker.len();
        let Some(offset) = find_subslice(&self.bytes[start..], marker) else { return Err(()) };
        self.position = start + offset + marker.len();
        Ok(())
    }

    fn parse_element(&mut self, depth: usize) -> Result<XmlNode, ()> {
        if depth > 64 {
            return Err(());
        }
        if self.peek() != Some(b'<') {
            return Err(());
        }
        self.position += 1;
        let tag = self.parse_name()?;
        // атрибуты разбираем и выбрасываем, но кавычки в них уважаем.
        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(b'/') => {
                    self.position += 1;
                    if self.peek() != Some(b'>') {
                        return Err(());
                    }
                    self.position += 1;
                    return Ok(XmlNode { tag, text: String::new(), children: Vec::new() });
                }
                Some(b'>') => {
                    self.position += 1;
                    break;
                }
                Some(_) => {
                    self.parse_name()?;
                    self.skip_whitespace();
                    if self.peek() != Some(b'=') {
                        return Err(());
                    }
                    self.position += 1;
                    self.skip_whitespace();
                    let quote = self.peek().ok_or(())?;
                    if quote != b'"' && quote != b'\'' {
                        return Err(());
                    }
                    self.position += 1;
                    while self.peek() != Some(quote) {
                        if self.peek().is_none() {
                            return Err(());
                        }
                        self.position += 1;
                    }
                    self.position += 1;
                }
                None => return Err(()),
            }
        }

        let mut text = String::new();
        let mut children = Vec::new();
        loop {
            if self.starts_with(b"</") {
                self.position += 2;
                let closing = self.parse_name()?;
                if closing != tag {
                    return Err(());
                }
                self.skip_whitespace();
                if self.peek() != Some(b'>') {
                    return Err(());
                }
                self.position += 1;
                return Ok(XmlNode { tag, text, children });
            }
            if self.starts_with(b"<![CDATA[") {
                self.position += 9;
                let start = self.position;
                let Some(offset) = find_subslice(&self.bytes[start..], b"]]>") else { return Err(()) };
                if children.is_empty() {
                    text.push_str(&String::from_utf8_lossy(&self.bytes[start..start + offset]));
                }
                self.position = start + offset + 3;
                continue;
            }
            if self.starts_with(b"<!--") {
                self.skip_until(b"-->")?;
                continue;
            }
            if self.starts_with(b"<!") || self.starts_with(b"<?") {
                // DOCTYPE, ENTITY и инструкции — запрещены как у defusedxml.
                return Err(());
            }
            if self.peek() == Some(b'<') {
                children.push(self.parse_element(depth + 1)?);
                continue;
            }
            let chunk = self.parse_text_chunk()?;
            if children.is_empty() {
                text.push_str(&chunk);
            }
        }
    }

    fn parse_name(&mut self) -> Result<String, ()> {
        let start = self.position;
        while matches!(self.peek(), Some(byte) if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b':' | b'.' | b'-')) {
            self.position += 1;
        }
        if self.position == start {
            return Err(());
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.position]).into_owned())
    }

    fn parse_text_chunk(&mut self) -> Result<String, ()> {
        let mut out = String::new();
        while let Some(byte) = self.peek() {
            if byte == b'<' {
                return Ok(out);
            }
            self.position += 1;
            if byte == b'&' {
                let entity = self.parse_entity()?;
                out.push_str(&entity);
            } else {
                out.push(byte as char);
            }
        }
        Err(())
    }

    fn parse_entity(&mut self) -> Result<String, ()> {
        let start = self.position;
        let Some(semicolon) = find_subslice(&self.bytes[start..], b";") else { return Err(()) };
        let entity = &self.bytes[start..start + semicolon];
        let decoded = match entity {
            b"amp" => "&".to_owned(),
            b"lt" => "<".to_owned(),
            b"gt" => ">".to_owned(),
            b"quot" => "\"".to_owned(),
            b"apos" => "'".to_owned(),
            _ if entity.first() == Some(&b'#') => {
                let code = if entity.get(1) == Some(&b'x') || entity.get(1) == Some(&b'X') {
                    u32::from_str_radix(&String::from_utf8_lossy(&entity[2..]), 16).map_err(|_| ())?
                } else {
                    u32::from_str_radix(&String::from_utf8_lossy(&entity[1..]), 10).map_err(|_| ())?
                };
                char::from_u32(code).ok_or(())?.to_string()
            }
            _ => return Err(()), // неопределённые сущности — как запрет DTD
        };
        self.position = start + semicolon + 1;
        Ok(decoded)
    }
}

/// xml.sax.saxutils.escape: только &, < и >.
pub(crate) fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

pub(crate) fn ssdp_locations(timeout: f64) -> UpnpResult<Vec<(String, String)>> {
    let packet = concat!(
        "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\n",
        "MAN: \"ssdp:discover\"\r\nMX: 1\r\nST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n"
    );
    let packet_v2 = packet.replace("InternetGatewayDevice:1", "InternetGatewayDevice:2");
    let deadline = Instant::now() + Duration::from_secs_f64(timeout.clamp(0.1, 4.0));

    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|_| UpnpError::new("UPnP-поиск недоступен. Проверьте локальную сеть и firewall"))?;
    #[cfg(unix)]
    {
        // TTL=1: multicast не должен покидать локальную сеть.
        use std::os::fd::AsRawFd;
        let ttl: libc::c_int = 1;
        unsafe {
            let fd = socket.as_raw_fd();
            libc::setsockopt(fd, libc::IPPROTO_IP, libc::IP_MULTICAST_TTL, &ttl as *const _ as *const libc::c_void, std::mem::size_of::<libc::c_int>() as u32);
        }
    }
    let target = SSDP_MULTICAST.parse::<SocketAddr>().expect("валидный SSDP-адрес");
    socket
        .send_to(packet.as_bytes(), target)
        .and_then(|_| socket.send_to(packet_v2.as_bytes(), target))
        .map_err(|_| UpnpError::new("UPnP-поиск недоступен. Проверьте локальную сеть и firewall"))?;

    let mut result: Vec<(String, String)> = Vec::new();
    // ограничены и время, и число пакетов — включая мусорные ответы из LAN.
    for _ in 0..64 {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if socket.set_read_timeout(Some(remaining)).is_err() {
            break;
        }
        let mut buffer = [0u8; 8192];
        let (count, sender) = match socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock || error.kind() == std::io::ErrorKind::TimedOut => break,
            Err(_) => break,
        };
        let peer = match sender.ip() {
            std::net::IpAddr::V4(address) => address.to_string(),
            std::net::IpAddr::V6(_) => continue,
        };
        if !lan_ipv4(&peer) {
            continue;
        }
        // iso-8859-1: каждый байт — символ.
        let raw: String = buffer[..count].iter().map(|byte| *byte as char).collect();
        let mut lines = raw.split("\r\n");
        if !lines.next().unwrap_or("").starts_with("HTTP/1.1 200") {
            continue;
        }
        let mut location = String::new();
        for line in lines {
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("location") && location.is_empty() {
                    location = value.trim().to_owned();
                }
            }
        }
        if location.is_empty() || router_url(&location, &peer).is_err() {
            continue;
        }
        let item = (location, peer);
        if !result.contains(&item) {
            result.push(item);
        }
        if result.len() >= 4 {
            break;
        }
    }
    Ok(result)
}

#[derive(Debug, Clone)]
pub(crate) struct Gateway {
    pub peer: String,
    pub control_url: String,
    pub service_type: String,
    pub name: String,
    pub local_ip: String,
}

impl Gateway {
    /// имена аргументов задаются этим модулем, а не XML от роутера.
    pub(crate) fn action(&self, action: &str, values: &[(&str, &str)]) -> UpnpResult<Vec<(String, String)>> {
        let elements: String = values
            .iter()
            .map(|(key, value)| format!("<{key}>{}</{key}>", xml_escape(value)))
            .collect();
        let body = format!(
            "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"{SOAP_NS}\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{}\">{elements}</u:{action}></s:Body></s:Envelope>",
            self.service_type
        );
        let soap_action = format!("\"{}#{}\"", self.service_type, action);
        let (status, raw) = router_request(
            &self.control_url,
            &self.peer,
            Some(body.as_bytes()),
            &[("Content-Type", "text/xml; charset=\"utf-8\""), ("SOAPAction", &soap_action)],
        )?;
        let root = parse_xml(&raw)?;
        let code = root.first_text("errorCode").unwrap_or_default();
        if status != 200 || !code.is_empty() {
            let hints = [
                ("718", "Внешний порт уже занят другим правилом роутера"),
                ("725", "Роутер поддерживает только постоянные правила. Используйте ручной проброс"),
                ("606", "Роутер запретил UPnP-проброс"),
                ("401", "Роутер не поддерживает эту UPnP-команду"),
            ];
            let fallback = format!("Ошибка UPnP {}", if code.is_empty() { status.to_string() } else { code.clone() });
            let message = hints.iter().find(|(known, _)| *known == code).map(|(_, text)| *text).unwrap_or(&fallback);
            return Err(UpnpError::with_code(message, &code));
        }
        let mut nodes = Vec::new();
        root.iter_nodes(&mut nodes);
        let response = nodes
            .into_iter()
            .find(|node| node.tag == format!("{action}Response"))
            .ok_or_else(|| UpnpError::new("Некорректный SOAP-ответ роутера"))?;
        Ok(response
            .children
            .iter()
            .map(|child| (child.tag.clone(), child.text.trim().to_owned()))
            .collect())
    }

    fn action_value(&self, action: &str, values: &[(&str, &str)], key: &str) -> UpnpResult<String> {
        let response = self.action(action, values)?;
        Ok(response
            .into_iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
            .unwrap_or_default())
    }

    pub(crate) fn external_ip(&self) -> UpnpResult<String> {
        let value = self.action_value("GetExternalIPAddress", &[], "NewExternalIPAddress")?;
        parse_ipv4(&value)
            .map(|address| address.to_string())
            .ok_or_else(|| UpnpError::new("Роутер не вернул корректный WAN IPv4"))
    }

    pub(crate) fn mapping(&self, port: u16) -> UpnpResult<Option<Vec<(String, String)>>> {
        match self.action(
            "GetSpecificPortMappingEntry",
            &[("NewRemoteHost", ""), ("NewExternalPort", &port.to_string()), ("NewProtocol", "TCP")],
        ) {
            Ok(response) => Ok(Some(response)),
            Err(error) if error.code == "714" => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn add(&self, external_port: u16, internal_port: u16, description: &str, lease: u32) -> UpnpResult<()> {
        self.action(
            "AddPortMapping",
            &[
                ("NewRemoteHost", ""),
                ("NewExternalPort", &external_port.to_string()),
                ("NewProtocol", "TCP"),
                ("NewInternalPort", &internal_port.to_string()),
                ("NewInternalClient", &self.local_ip),
                ("NewEnabled", "1"),
                ("NewPortMappingDescription", description),
                ("NewLeaseDuration", &lease.to_string()),
            ],
        )
        .map(|_| ())
    }

    pub(crate) fn delete(&self, port: u16) -> UpnpResult<()> {
        match self.action(
            "DeletePortMapping",
            &[("NewRemoteHost", ""), ("NewExternalPort", &port.to_string()), ("NewProtocol", "TCP")],
        ) {
            Ok(_) => Ok(()),
            Err(error) if error.code == "714" => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// только UDP-маршрутизация; connect() не отправляет пакеты.
pub(crate) fn source_ip(peer: &str) -> String {
    let port = if lan_ipv4(peer) { 1900 } else { 443 };
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0") else { return String::new() };
    let address = SocketAddr::V4(SocketAddrV4::new(
        parse_ipv4(peer).unwrap_or(Ipv4Addr::new(1, 1, 1, 1)),
        port,
    ));
    if socket.connect(address).is_err() {
        return String::new();
    }
    socket
        .local_addr()
        .ok()
        .and_then(|address| match address {
            SocketAddr::V4(address) => Some(address.ip().to_string()),
            SocketAddr::V6(_) => None,
        })
        .unwrap_or_default()
}

pub(crate) fn gateways_from_description(raw: &[u8], location: &str, peer: &str) -> UpnpResult<Vec<Gateway>> {
    router_url(location, peer)?;
    let root = parse_xml(raw)?;
    let friendly = root
        .first_text("friendlyName")
        .map(|name| name.chars().take(80).collect::<String>())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "UPnP Router".to_owned());
    let mut base = location.to_owned();
    let url_base = child_text(&root, "URLBase");
    if !url_base.is_empty() && router_url(&url_base, peer).is_ok() {
        base = url_base;
    }
    let local_ip = source_ip(peer);
    if !lan_ipv4(&local_ip) {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for service_type in SERVICE_TYPES {
        for service in root.find_all("service") {
            if child_text(service, "serviceType") != *service_type {
                continue;
            }
            let control = child_text(service, "controlURL");
            let url = Url::parse(&base)
                .and_then(|base| base.join(&control))
                .map(|joined| joined.to_string())
                .map_err(|_| UpnpError::new("UPnP может обращаться только к локальному роутеру, ответившему на SSDP"))?;
            router_url(&url, peer)?;
            result.push(Gateway {
                peer: peer.to_owned(),
                control_url: url,
                service_type: service_type.to_string(),
                name: friendly.clone(),
                local_ip: local_ip.clone(),
            });
        }
    }
    Ok(result)
}

pub(crate) fn discover_gateway() -> UpnpResult<Gateway> {
    for (location, peer) in ssdp_locations(2.0)? {
        let attempt = (|| -> UpnpResult<Option<Gateway>> {
            let (status, raw) = router_request(&location, &peer, None, &[])?;
            if status != 200 {
                return Ok(None);
            }
            for gateway in gateways_from_description(&raw, &location, &peer)?.into_iter().take(3) {
                if gateway.external_ip().is_ok() {
                    return Ok(Some(gateway));
                }
            }
            Ok(None)
        })();
        if let Ok(Some(gateway)) = attempt {
            return Ok(gateway);
        }
    }
    Err(UpnpError::new("UPnP IGD не найден. Включите UPnP на роутере или настройте проброс вручную"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_parser_matches_python_strictness() {
        assert_eq!(parse_ipv4("192.168.1.1"), Some(Ipv4Addr::new(192, 168, 1, 1)));
        assert_eq!(parse_ipv4("10.0.0.255"), Some(Ipv4Addr::new(10, 0, 0, 255)));
        assert_eq!(parse_ipv4("01.2.3.4"), None, "ведущие нули отвергаются");
        assert_eq!(parse_ipv4("1.2.3"), None);
        assert_eq!(parse_ipv4("1.2.3.4.5"), None);
        assert_eq!(parse_ipv4("1.2.3.256"), None);
        assert_eq!(parse_ipv4(" 1.2.3.4"), None);
        assert_eq!(parse_ipv4("localhost"), None);
        assert_eq!(parse_ipv4("::1"), None);
    }

    #[test]
    fn lan_check_covers_rfc1918_only() {
        assert!(lan_ipv4("10.1.2.3"));
        assert!(lan_ipv4("172.16.0.1"));
        assert!(lan_ipv4("172.31.255.255"));
        assert!(!lan_ipv4("172.32.0.1"));
        assert!(lan_ipv4("192.168.0.1"));
        assert!(!lan_ipv4("8.8.8.8"));
        assert!(!lan_ipv4("127.0.0.1"));
    }

    #[test]
    fn router_url_gates_untrusted_locations() {
        assert_eq!(router_url("http://192.168.1.1:49152/rootDesc.xml", "192.168.1.1").unwrap().1, 49152);
        let (_, port, path) = router_url("http://10.0.0.138/desc.xml?x=1", "10.0.0.138").unwrap();
        assert_eq!((port, path.as_str()), (80, "/desc.xml?x=1"));
        // хост не равен пиру.
        assert!(router_url("http://192.168.1.2/rootDesc.xml", "192.168.1.1").is_err());
        // не LAN-адрес.
        assert!(router_url("http://8.8.8.8/rootDesc.xml", "8.8.8.8").is_err());
        // HTTPS, учётные данные, фрагмент, кривой порт — мимо.
        assert!(router_url("https://192.168.1.1/x", "192.168.1.1").is_err());
        assert!(router_url("http://user:pass@192.168.1.1/x", "192.168.1.1").is_err());
        assert!(router_url("http://192.168.1.1/x#frag", "192.168.1.1").is_err());
        assert!(router_url("http://192.168.1.1:99999/x", "192.168.1.1").is_err());
        assert!(router_url("http://192.168.1.1/\u{1}", "192.168.1.1").is_err());
        assert!(router_url(&format!("http://192.168.1.1/{}", "a".repeat(2048)), "192.168.1.1").is_err());
    }

    #[test]
    fn http_response_parsing_supports_length_and_chunked() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcextra";
        let response = parse_http_response(plain).unwrap();
        assert_eq!((response.status, response.body.as_slice()), (200, b"abc".as_slice()));

        let chunked = b"HTTP/1.1 500 Internal\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nabcd\r\n3\r\nefg\r\n0\r\n\r\n";
        let response = parse_http_response(chunked).unwrap();
        assert_eq!((response.status, response.body.as_slice()), (500, b"abcdefg".as_slice()));

        let closed = b"HTTP/1.1 200 OK\r\n\r\nno-length";
        let response = parse_http_response(closed).unwrap();
        assert_eq!(response.body.as_slice(), b"no-length".as_slice());
    }

    #[test]
    fn xml_rejects_dtd_and_unknown_entities() {
        let ok = parse_xml(b"<root><a>718</a><b x=\"1\"/></root>").unwrap();
        assert_eq!(child_text(&ok, "a"), "718");
        assert_eq!(ok.first_text("a").unwrap(), "718");
        assert!(ok.find_all("b").len() == 1);

        let namespaced = parse_xml(b"<r xmlns=\"urn:x\"><errorCode>606</errorCode></r>").unwrap();
        assert_eq!(namespaced.first_text("errorCode").unwrap(), "606");

        assert!(parse_xml(b"<!DOCTYPE r [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]><r>&xxe;</r>").is_err());
        assert!(parse_xml(b"<r>&unknown;</r>").is_err());
        // незакрытый CDATA — обрыв документа.
        assert!(parse_xml(b"<r><![CDATA[unterminated</r>").is_err());
        assert_eq!(
            parse_xml("<r><![CDATA[значение]]></r>".as_bytes()).unwrap().text,
            "значение"
        );
        assert_eq!(parse_xml(b"<r>&#1044;&#x430;</r>").unwrap().text, "Да");
        assert!(parse_xml(b"<r><a></b></r>").is_err());
    }

    #[test]
    fn gateway_description_builds_lan_gateways() {
        let description = concat!(
            "<?xml version=\"1.0\"?>",
            "<root xmlns=\"urn:schemas-upnp-org:device-1-0\">",
            "<device><friendlyName>Keenetic</friendlyName>",
            "<serviceList><service>",
            "<serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>",
            "<controlURL>/ctl/IPConn</controlURL>",
            "</service></serviceList></device></root>"
        );
        // source_ip зависит от маршрута — на CI шлюзы не проверяем, только разбор.
        let root = parse_xml(description.as_bytes()).unwrap();
        assert_eq!(root.first_text("friendlyName").unwrap(), "Keenetic");
        let services = root.find_all("service");
        assert_eq!(services.len(), 1);
        assert_eq!(child_text(services[0], "controlURL"), "/ctl/IPConn");
    }

    #[test]
    fn xml_escape_covers_only_three_characters() {
        assert_eq!(xml_escape("a<b>&\"'"), "a&lt;b&gt;&amp;\"'");
        assert_eq!(xml_escape("текст"), "текст");
    }
}
