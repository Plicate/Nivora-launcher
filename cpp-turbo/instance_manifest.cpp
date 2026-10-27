#include "instance_manifest.hpp"

#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <fstream>
#include <memory>
#include <string>
#include <thread>
#include <vector>
#include <zlib.h>

#if defined(_WIN32)
#include <windows.h>
#else
#include <sys/stat.h>
#include <dirent.h>
#endif

namespace {

struct Sha256 {
    uint32_t h[8];
    uint64_t total;
    unsigned char buffer[64];
    size_t buffered;

    Sha256() { reset(); }

    void reset() {
        static const uint32_t INIT[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                         0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
        for (int i = 0; i < 8; i++) h[i] = INIT[i];
        total = 0;
        buffered = 0;
    }

    static uint32_t rotr(uint32_t v, unsigned b) { return (v >> b) | (v << (32 - b)); }

    void compress(const unsigned char *p) {
        static const uint32_t K[64] = {
            0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
            0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
            0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
            0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
            0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
            0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
            0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
            0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
        };
        uint32_t w[64];
        for (int i = 0; i < 16; i++) {
            w[i] = (uint32_t(p[4 * i]) << 24) | (uint32_t(p[4 * i + 1]) << 16) |
                   (uint32_t(p[4 * i + 2]) << 8) | uint32_t(p[4 * i + 3]);
        }
        for (int i = 16; i < 64; i++) {
            uint32_t s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
            uint32_t s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16] + s0 + w[i - 7] + s1;
        }
        uint32_t a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7];
        for (int i = 0; i < 64; i++) {
            uint32_t s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
            uint32_t ch = (e & f) ^ (~e & g);
            uint32_t t1 = hh + s1 + ch + K[i] + w[i];
            uint32_t s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
            uint32_t maj = (a & b) ^ (a & c) ^ (b & c);
            uint32_t t2 = s0 + maj;
            hh = g; g = f; f = e; e = d + t1; d = c; c = b; b = a; a = t1 + t2;
        }
        h[0] += a; h[1] += b; h[2] += c; h[3] += d;
        h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
    }

    void update(const void *data, size_t length) {
        const unsigned char *p = static_cast<const unsigned char *>(data);
        total += length;
        while (length > 0) {
            size_t take = 64 - buffered;
            if (take > length) take = length;
            std::memcpy(buffer + buffered, p, take);
            buffered += take;
            p += take;
            length -= take;
            if (buffered == 64) {
                compress(buffer);
                buffered = 0;
            }
        }
    }

    void finalize(unsigned char out[32]) {
        uint64_t bits = total * 8;
        unsigned char pad = 0x80;
        update(&pad, 1);
        unsigned char zero = 0;
        while (buffered != 56) update(&zero, 1);
        unsigned char length_bytes[8];
        for (int i = 0; i < 8; i++) length_bytes[i] = static_cast<unsigned char>(bits >> (56 - 8 * i));
        total = 0;  // байты длины не входят в счётчик (он уже зафиксирован)
        update(length_bytes, 8);
        for (int i = 0; i < 8; i++) {
            out[4 * i] = static_cast<unsigned char>(h[i] >> 24);
            out[4 * i + 1] = static_cast<unsigned char>(h[i] >> 16);
            out[4 * i + 2] = static_cast<unsigned char>(h[i] >> 8);
            out[4 * i + 3] = static_cast<unsigned char>(h[i]);
        }
    }
};

std::string hex32(const unsigned char digest[32]) {
    static const char *DIGITS = "0123456789abcdef";
    std::string out;
    out.reserve(64);
    for (int i = 0; i < 32; i++) {
        out.push_back(DIGITS[digest[i] >> 4]);
        out.push_back(DIGITS[digest[i] & 0x0f]);
    }
    return out;
}

std::string join_path(const std::string &base, const std::string &name) {
    if (base.empty()) return name;
    char last = base[base.size() - 1];
#if defined(_WIN32)
    if (last != '/' && last != '\\') return base + "/" + name;
#else
    if (last != '/') return base + "/" + name;
#endif
    return base + name;
}

bool is_directory(const std::string &path) {
#if defined(_WIN32)
    DWORD attributes = GetFileAttributesA(path.c_str());
    return attributes != INVALID_FILE_ATTRIBUTES && (attributes & FILE_ATTRIBUTE_DIRECTORY);
#else
    struct stat info {};
    return ::stat(path.c_str(), &info) == 0 && S_ISDIR(info.st_mode);
#endif
}

bool is_regular_file(const std::string &path) {
#if defined(_WIN32)
    DWORD attributes = GetFileAttributesA(path.c_str());
    return attributes != INVALID_FILE_ATTRIBUTES && !(attributes & FILE_ATTRIBUTE_DIRECTORY);
#else
    struct stat info {};
    return ::stat(path.c_str(), &info) == 0 && S_ISREG(info.st_mode);
#endif
}

uint64_t file_size_of(const std::string &path) {
#if defined(_WIN32)
    HANDLE file = CreateFileA(path.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
    if (file == INVALID_HANDLE_VALUE) return 0;
    LARGE_INTEGER size{};
    BOOL ok = GetFileSizeEx(file, &size);
    CloseHandle(file);
    return ok ? static_cast<uint64_t>(size.QuadPart) : 0;
#else
    struct stat info {};
    return ::stat(path.c_str(), &info) == 0 ? static_cast<uint64_t>(info.st_size) : 0;
#endif
}

bool read_whole_file(const std::string &path, size_t limit, std::string *out) {
    std::ifstream stream(path, std::ios::binary);
    if (!stream) return false;
    stream.seekg(0, std::ios::end);
    std::streamoff size = stream.tellg();
    if (size < 0 || static_cast<uint64_t>(size) > limit) return false;
    stream.seekg(0, std::ios::beg);
    out->resize(static_cast<size_t>(size));
    if (size > 0) stream.read(&(*out)[0], size);
    return static_cast<bool>(stream) || size == 0;
}

std::string sha256_file(const std::string &path, uint64_t *size_out) {
    // хешируем файл блоками по 1 МиБ; "-" означает ошибку чтения
    std::ifstream stream(path, std::ios::binary);
    if (!stream) return "-";
    Sha256 sha;
    std::vector<char> buffer(1024 * 1024);
    uint64_t total = 0;
    while (stream) {
        stream.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
        std::streamsize got = stream.gcount();
        if (got <= 0) break;
        sha.update(buffer.data(), static_cast<size_t>(got));
        total += static_cast<uint64_t>(got);
    }
    if (stream.bad()) return "-";
    unsigned char digest[32];
    sha.finalize(digest);
    if (size_out) *size_out = total;
    return hex32(digest);
}

std::string to_lower(const std::string &value) {
    std::string out = value;
    for (char &c : out)
        if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
    return out;
}

bool ends_with(const std::string &value, const std::string &suffix) {
    return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}

// возвращает имена записей без "."/".." в алфавитном порядке
std::vector<std::string> list_directory(const std::string &path) {
    std::vector<std::string> names;
#if defined(_WIN32)
    std::string pattern = join_path(path, "*");
    WIN32_FIND_DATAA data{};
    HANDLE finder = FindFirstFileA(pattern.c_str(), &data);
    if (finder == INVALID_HANDLE_VALUE) return names;
    do {
        const char *name = data.cFileName;
        if (std::strcmp(name, ".") == 0 || std::strcmp(name, "..") == 0) continue;
        if (data.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) continue;  // symlink/junction
        names.push_back(name);
    } while (FindNextFileA(finder, &data));
    FindClose(finder);
#else
    // обходим каталог через opendir/readdir
    DIR *dir = ::opendir(path.c_str());
    if (dir == nullptr) return names;
    while (struct dirent *entry = ::readdir(dir)) {
        const char *name = entry->d_name;
        if (std::strcmp(name, ".") == 0 || std::strcmp(name, "..") == 0) continue;
        std::string full = join_path(path, name);
        struct stat info {};
        if (::lstat(full.c_str(), &info) != 0) continue;
        if (S_ISLNK(info.st_mode)) continue;  // символьные ссылки пропускаем
        names.push_back(name);
    }
    ::closedir(dir);
#endif
    std::sort(names.begin(), names.end());
    return names;
}

struct ZipEntryInfo {
    std::string name;
    uint64_t compressed_size;
    uint64_t uncompressed_size;
    uint32_t crc32;
    uint64_t local_header_offset;
    uint16_t method;
};

uint16_t read_u16(const unsigned char *p) { return uint16_t(p[0]) | (uint16_t(p[1]) << 8); }
uint32_t read_u32(const unsigned char *p) {
    return uint32_t(p[0]) | (uint32_t(p[1]) << 8) | (uint32_t(p[2]) << 16) | (uint32_t(p[3]) << 24);
}

// формат ZIP64 не поддерживается; архив ограничен 2 ГиБ
bool read_zip_entries(const std::string &path, std::vector<ZipEntryInfo> *entries) {
    std::ifstream stream(path, std::ios::binary);
    if (!stream) return false;
    stream.seekg(0, std::ios::end);
    std::streamoff file_size = stream.tellg();
    if (file_size < 22 || file_size > 2LL * 1024 * 1024 * 1024) return false;
    // запись EOCD ищем с конца; комментарий ≤ 65535 байт
    std::streamoff scan_from = file_size - 22 - 65535;
    if (scan_from < 0) scan_from = 0;
    std::streamoff scan_length = file_size - scan_from;
    std::string tail(static_cast<size_t>(scan_length), '\0');
    stream.seekg(scan_from, std::ios::beg);
    stream.read(&tail[0], static_cast<std::streamsize>(scan_length));
    std::streamoff eocd_at = -1;
    for (std::streamoff i = scan_length - 22; i >= 0; i--) {
        if (std::memcmp(tail.data() + i, "PK\x05\x06", 4) == 0) {
            eocd_at = scan_from + i;
            break;
        }
    }
    if (eocd_at < 0) return false;
    std::string eocd(22, '\0');
    stream.seekg(eocd_at, std::ios::beg);
    stream.read(&eocd[0], 22);
    const unsigned char *e = reinterpret_cast<const unsigned char *>(eocd.data());
    uint32_t entry_count = read_u16(e + 10);
    uint32_t central_size = read_u32(e + 12);
    uint32_t central_offset = read_u32(e + 16);
    if (central_size > 64u * 1024 * 1024 || entry_count > 200000) return false;
    if (static_cast<uint64_t>(central_offset) + central_size > static_cast<uint64_t>(eocd_at)) return false;
    std::string central(central_size, '\0');
    stream.seekg(central_offset, std::ios::beg);
    stream.read(&central[0], static_cast<std::streamsize>(central_size));
    if (!stream && central_size > 0) return false;
    entries->clear();
    entries->reserve(entry_count);
    size_t offset = 0;
    const unsigned char *c = reinterpret_cast<const unsigned char *>(central.data());
    for (uint32_t index = 0; index < entry_count; index++) {
        if (offset + 46 > central.size() || std::memcmp(c + offset, "PK\x01\x02", 4) != 0) return false;
        ZipEntryInfo info{};
        info.method = read_u16(c + offset + 10);
        info.crc32 = read_u32(c + offset + 16);
        info.compressed_size = read_u32(c + offset + 20);
        info.uncompressed_size = read_u32(c + offset + 24);
        uint16_t name_length = read_u16(c + offset + 28);
        uint16_t extra_length = read_u16(c + offset + 30);
        uint16_t comment_length = read_u16(c + offset + 32);
        info.local_header_offset = read_u32(c + offset + 42);
        if (offset + 46 + name_length > central.size()) return false;
        info.name.assign(central, offset + 46, name_length);
        entries->push_back(info);
        offset += 46 + static_cast<size_t>(name_length) + extra_length + comment_length;
    }
    return true;
}

// распакованный размер записи не должен превышать limit
std::string read_zip_entry(std::ifstream &stream, const ZipEntryInfo &info, size_t limit) {
    if (info.uncompressed_size > limit) return {};
    stream.seekg(static_cast<std::streamoff>(info.local_header_offset), std::ios::beg);
    if (!stream) return {};
    unsigned char header[30];
    stream.read(reinterpret_cast<char *>(header), 30);
    if (!stream || std::memcmp(header, "PK\x03\x04", 4) != 0) return {};
    uint16_t name_length = read_u16(header + 26);
    uint16_t extra_length = read_u16(header + 28);
    stream.seekg(name_length + extra_length, std::ios::cur);
    if (!stream) return {};
    std::string compressed(static_cast<size_t>(info.compressed_size), '\0');
    if (info.compressed_size > 0) {
        stream.read(&compressed[0], static_cast<std::streamsize>(info.compressed_size));
        if (!stream) return {};
    }
    if (info.method == 0) return compressed;  // stored
    if (info.method != 8) return {};          // только deflate
    std::string out(static_cast<size_t>(info.uncompressed_size), '\0');
    z_stream zs{};
    if (inflateInit2(&zs, -15) != Z_OK) return {};
    zs.next_in = reinterpret_cast<Bytef *>(&compressed[0]);
    zs.avail_in = static_cast<uInt>(compressed.size());
    zs.next_out = reinterpret_cast<Bytef *>(&out[0]);
    zs.avail_out = static_cast<uInt>(out.size());
    int status = inflate(&zs, Z_FINISH);
    inflateEnd(&zs);
    if (status != Z_STREAM_END) return {};
    return out;
}

// ищет строковые поля в плоском JSON манифеста
std::string json_string_field(const std::string &text, const std::string &key) {
    std::string needle = "\"" + key + "\"";
    size_t at = 0;
    while (true) {
        size_t key_at = text.find(needle, at);
        if (key_at == std::string::npos) return {};
        size_t cursor = key_at + needle.size();
        while (cursor < text.size() && std::isspace(static_cast<unsigned char>(text[cursor]))) cursor++;
        if (cursor >= text.size() || text[cursor] != ':') {
            at = key_at + needle.size();
            continue;
        }
        cursor++;
        while (cursor < text.size() && std::isspace(static_cast<unsigned char>(text[cursor]))) cursor++;
        if (cursor >= text.size() || text[cursor] != '"') {
            at = key_at + needle.size();
            continue;
        }
        cursor++;
        std::string value;
        while (cursor < text.size() && text[cursor] != '"') {
            char c = text[cursor];
            if (c == '\\' && cursor + 1 < text.size()) {
                cursor++;
                char escaped = text[cursor];
                switch (escaped) {
                    case 'n': value.push_back('\n'); break;
                    case 't': value.push_back('\t'); break;
                    case 'r': value.push_back('\r'); break;
                    case '"': value.push_back('"'); break;
                    case '\\': value.push_back('\\'); break;
                    case '/': value.push_back('/'); break;
                    case 'u': {
                        if (cursor + 4 < text.size()) {
                            unsigned code = 0;
                            for (int digit = 1; digit <= 4; digit++) {
                                char hex = text[cursor + static_cast<size_t>(digit)];
                                code <<= 4;
                                if (hex >= '0' && hex <= '9') code |= unsigned(hex - '0');
                                else if (hex >= 'a' && hex <= 'f') code |= unsigned(hex - 'a' + 10);
                                else if (hex >= 'A' && hex <= 'F') code |= unsigned(hex - 'A' + 10);
                                else { code = 0; break; }
                            }
                            // декодируем BMP-коды в UTF-8
                            if (code > 0 && code < 0x80) {
                                value.push_back(static_cast<char>(code));
                            } else {
                                // формируем многобайтовую последовательность UTF-8
                                if (code < 0x800) {
                                    value.push_back(static_cast<char>(0xC0 | (code >> 6)));
                                    value.push_back(static_cast<char>(0x80 | (code & 0x3F)));
                                } else {
                                    value.push_back(static_cast<char>(0xE0 | (code >> 12)));
                                    value.push_back(static_cast<char>(0x80 | ((code >> 6) & 0x3F)));
                                    value.push_back(static_cast<char>(0x80 | (code & 0x3F)));
                                }
                            }
                            cursor += 4;
                        }
                        break;
                    }
                    default: value.push_back(escaped); break;
                }
                cursor++;
            } else {
                value.push_back(c);
                cursor++;
            }
        }
        return value;
    }
}

std::string toml_string_field(const std::string &text, const std::string &key) {
    // читает строки вида key="value" или key = 'value'
    size_t at = 0;
    while (true) {
        size_t line_start = text.find('\n', at);
        std::string line = text.substr(at, line_start == std::string::npos ? std::string::npos : line_start - at);
        size_t key_at = line.find(key);
        if (key_at != std::string::npos) {
            size_t cursor = key_at + key.size();
            while (cursor < line.size() && std::isspace(static_cast<unsigned char>(line[cursor]))) cursor++;
            if (cursor < line.size() && line[cursor] == '=') {
                cursor++;
                while (cursor < line.size() && std::isspace(static_cast<unsigned char>(line[cursor]))) cursor++;
                if (cursor < line.size() && (line[cursor] == '"' || line[cursor] == '\'')) {
                    char quote = line[cursor++];
                    size_t end = line.find(quote, cursor);
                    if (end != std::string::npos) return line.substr(cursor, end - cursor);
                }
            }
        }
        if (line_start == std::string::npos) return {};
        at = line_start + 1;
    }
}

struct ModManifest {
    std::string loader;   // fabric | quilt | forge | neoforge | legacy | "-"
    std::string mod_id;
    std::string name;
    std::string version;
};

ModManifest parse_mod_manifest(const std::string &entry_name, const std::string &raw) {
    ModManifest result;
    std::string lowered = to_lower(entry_name);
    if (lowered == "fabric.mod.json") {
        result.loader = "fabric";
        result.mod_id = json_string_field(raw, "id");
        result.version = json_string_field(raw, "version");
        result.name = json_string_field(raw, "name");
    } else if (lowered == "quilt.mod.json") {
        result.loader = "quilt";
        result.mod_id = json_string_field(raw, "id");
        result.version = json_string_field(raw, "version");
        result.name = json_string_field(raw, "name");
    } else if (lowered == "litemod.json") {
        result.loader = "legacy";
        result.mod_id = json_string_field(raw, "name");
        result.version = json_string_field(raw, "version");
        result.name = json_string_field(raw, "displayName");
    } else if (lowered == "mcmod.info") {
        result.loader = "legacy";
        result.mod_id = json_string_field(raw, "modid");
        result.version = json_string_field(raw, "version");
        result.name = json_string_field(raw, "name");
    } else if (lowered.find("mods.toml") != std::string::npos) {
        result.loader = lowered.find("neoforge") != std::string::npos ? "neoforge" : "forge";
        result.mod_id = toml_string_field(raw, "modId");
        result.version = toml_string_field(raw, "version");
        result.name = toml_string_field(raw, "displayName");
    }
    if (result.name.empty()) result.name = "-";
    if (result.mod_id.empty()) result.mod_id = "-";
    if (result.version.empty()) result.version = "-";
    return result;
}

struct FileRecord {
    char kind;  // варианты kind: M | P | S | C
    std::string relative;
    std::string absolute;
    uint64_t size;
};

struct ModStats {
    std::string sha256;
    std::string loader;
    std::string mod_id;
    std::string name;
    std::string version;
    uint32_t class_count;
    uint64_t unpacked_bytes;
};

struct PackStats {
    std::string sha256;
    uint32_t programs;  // только шейдерпаки: shaders/programs/*
    uint32_t files;
};

std::string normalize_relative(const std::string &prefix, const std::string &name) {
    // нормализуем разделители путей для diff на Windows и Linux
    std::string value = prefix.empty() ? name : prefix + "/" + name;
    for (char &c : value)
        if (c == '\\') c = '/';
    return value;
}

void scan_directory_tree(const std::string &absolute, const std::string &prefix, char kind,
                         std::vector<FileRecord> *records) {
    for (const std::string &name : list_directory(absolute)) {
        std::string full = join_path(absolute, name);
        std::string relative = normalize_relative(prefix, name);
        if (is_directory(full)) {
            scan_directory_tree(full, relative, kind, records);
        } else if (is_regular_file(full)) {
            FileRecord record;
            record.kind = kind;
            record.relative = relative;
            record.absolute = full;
            record.size = file_size_of(full);
            records->push_back(record);
        }
    }
}

void scan_mods(const std::string &root, std::vector<FileRecord> *records) {
    std::string mods = join_path(root, "mods");
    if (!is_directory(mods)) return;
    for (const std::string &name : list_directory(mods)) {
        std::string full = join_path(mods, name);
        std::string lowered = to_lower(name);
        bool disabled = ends_with(lowered, ".disabled");
        if (disabled) lowered = lowered.substr(0, lowered.size() - 9);
        bool jar = ends_with(lowered, ".jar") || ends_with(lowered, ".litemod");
        if (!is_regular_file(full) || !jar) continue;
        FileRecord record;
        record.kind = 'M';
        record.relative = normalize_relative("mods", name);
        record.absolute = full;
        record.size = file_size_of(full);
        (void)disabled;  // суффикс сохраняется в имени пути для вызывающей стороны
        records->push_back(record);
    }
}

void scan_configs(const std::string &root, std::vector<FileRecord> *records) {
    std::string config = join_path(root, "config");
    if (is_directory(config)) scan_directory_tree(config, "config", 'C', records);
    std::string options = join_path(root, "options.txt");
    if (is_regular_file(options)) {
        FileRecord record;
        record.kind = 'C';
        record.relative = "options.txt";
        record.absolute = options;
        record.size = file_size_of(options);
        records->push_back(record);
    }
}

// значения options.txt для сравнения и автонастройки
void scan_options_keys(const std::string &root) {
    static const char *INTERESTING[] = {
        "renderDistance", "simulationDistance", "graphicsMode", "fov", "guiScale",
        "maxFps", "mipmapLevels", "fullscreen", "particleStatus", "gamma", nullptr,
    };
    std::string options = join_path(root, "options.txt");
    std::string text;
    if (!read_whole_file(options, 256 * 1024, &text)) return;
    size_t at = 0;
    while (at < text.size()) {
        size_t line_end = text.find('\n', at);
        std::string line = text.substr(at, line_end == std::string::npos ? std::string::npos : line_end - at);
        at = line_end == std::string::npos ? text.size() : line_end + 1;
        size_t colon = line.find(':');
        if (colon == std::string::npos) continue;
        std::string key = line.substr(0, colon);
        std::string value = line.substr(colon + 1);
        while (!value.empty() && (value.back() == '\r' || value.back() == '\n')) value.pop_back();
        for (const char **candidate = INTERESTING; *candidate != nullptr; candidate++) {
            if (key == *candidate) {
                std::printf("O\t%s\t%s\n", key.c_str(), value.c_str());
                break;
            }
        }
    }
}

// выбирает метаданные каталога версии с максимальным временем изменения
void scan_version_meta(const std::string &root) {
    std::string versions = join_path(root, "versions");
    if (!is_directory(versions)) return;
    std::string best_id;
    std::string best_inherits;
    uint64_t best_mtime = 0;
    bool found = false;
    for (const std::string &name : list_directory(versions)) {
        std::string directory = join_path(versions, name);
        if (!is_directory(directory)) continue;
        std::string metadata = join_path(directory, name + ".json");
        if (!is_regular_file(metadata)) metadata = join_path(directory, name + "-.json");
        if (!is_regular_file(metadata)) {
            // если ожидаемого имени нет, берём первый JSON-файл
            for (const std::string &entry : list_directory(directory)) {
                if (ends_with(to_lower(entry), ".json") && is_regular_file(join_path(directory, entry))) {
                    metadata = join_path(directory, entry);
                    break;
                }
            }
        }
        if (!is_regular_file(metadata)) continue;
#if defined(_WIN32)
        HANDLE file = CreateFileA(metadata.c_str(), GENERIC_READ, FILE_SHARE_READ, nullptr, OPEN_EXISTING, 0, nullptr);
        if (file == INVALID_HANDLE_VALUE) continue;
        FILETIME write_time{};
        uint64_t mtime = 0;
        if (GetFileTime(file, nullptr, nullptr, &write_time)) {
            mtime = (uint64_t(write_time.dwHighDateTime) << 32) | write_time.dwLowDateTime;
        }
        CloseHandle(file);
#else
        struct stat info {};
        if (::stat(metadata.c_str(), &info) != 0) continue;
        uint64_t mtime = static_cast<uint64_t>(info.st_mtime);
#endif
        if (found && mtime <= best_mtime) continue;
        std::string text;
        if (!read_whole_file(metadata, 4 * 1024 * 1024, &text)) continue;
        std::string id = json_string_field(text, "id");
        if (id.empty()) id = name;
        found = true;
        best_mtime = mtime;
        best_id = id;
        best_inherits = json_string_field(text, "inheritsFrom");
    }
    if (found) {
        std::printf("V\tversion_id\t%s\n", best_id.c_str());
        if (!best_inherits.empty()) std::printf("V\tminecraft_version\t%s\n", best_inherits.c_str());
    }
}

ModStats analyze_mod(const FileRecord &record) {
    ModStats stats;
    stats.class_count = 0;
    stats.unpacked_bytes = 0;
    stats.loader = "-";
    stats.mod_id = "-";
    stats.name = "-";
    stats.version = "-";
    std::vector<ZipEntryInfo> entries;
    if (read_zip_entries(record.absolute, &entries)) {
        std::ifstream stream(record.absolute, std::ios::binary);
        for (const ZipEntryInfo &entry : entries) {
            stats.unpacked_bytes += entry.uncompressed_size;
            std::string lowered = to_lower(entry.name);
            if (ends_with(lowered, ".class")) stats.class_count++;
            if (!stats.loader.empty() && stats.loader != "-") continue;
            bool manifest = lowered == "fabric.mod.json" || lowered == "quilt.mod.json" ||
                            lowered == "litemod.json" || lowered == "mcmod.info" ||
                            lowered.find("mods.toml") != std::string::npos;
            if (!manifest) continue;
            std::string raw = read_zip_entry(stream, entry, 2 * 1024 * 1024);
            if (raw.empty()) continue;
            ModManifest parsed = parse_mod_manifest(entry.name, raw);
            if (parsed.loader != "-" || parsed.mod_id != "-") {
                stats.loader = parsed.loader;
                stats.mod_id = parsed.mod_id;
                stats.name = parsed.name;
                stats.version = parsed.version;
            }
        }
    }
    stats.sha256 = sha256_file(record.absolute, nullptr);
    return stats;
}

PackStats analyze_pack(const FileRecord &record) {
    PackStats stats;
    stats.programs = 0;
    stats.files = 0;
    std::vector<ZipEntryInfo> entries;
    if (read_zip_entries(record.absolute, &entries)) {
        stats.files = static_cast<uint32_t>(entries.size());
        for (const ZipEntryInfo &entry : entries) {
            std::string lowered = to_lower(entry.name);
            // учитываем shaders/programs и корневые GLSL-файлы шейдерпака
            if (lowered.find("shaders/programs/") == 0 || lowered.find("shaders\\programs\\") == 0) {
                if (ends_with(lowered, ".glsl") || ends_with(lowered, ".fsh") || ends_with(lowered, ".vsh") ||
                    ends_with(lowered, ".gsh") || ends_with(lowered, ".csh")) {
                    stats.programs++;
                }
            } else if (lowered.find("shaders/") == 0 || lowered.find("shaders\\") == 0) {
                if (ends_with(lowered, ".glsl")) stats.programs++;
            }
        }
    }
    stats.sha256 = sha256_file(record.absolute, nullptr);
    return stats;
}

std::string sanitize_tsv(const std::string &value) {
    // поля TSV не могут содержать табуляцию и переводы строк
    std::string out;
    out.reserve(value.size());
    for (char c : value) {
        if (c == '\t' || c == '\n' || c == '\r') out.push_back(' ');
        else out.push_back(c);
    }
    return out;
}

}  // namespace

namespace nivora {

int run_instance_manifest(const std::vector<std::string> &args) {
    if (args.size() != 1 || args[0].empty()) {
        std::fprintf(stderr, "использование: nivora-turbo instance-manifest DIR\n");
        return 2;
    }
    const std::string root = args[0];
    if (!is_directory(root)) {
        std::fprintf(stderr, "каталог экземпляра не читается: %s\n", root.c_str());
        return 1;
    }

    scan_version_meta(root);
    scan_options_keys(root);

    std::vector<FileRecord> records;
    scan_mods(root, &records);
    std::string resourcepacks = join_path(root, "resourcepacks");
    if (is_directory(resourcepacks)) scan_directory_tree(resourcepacks, "resourcepacks", 'P', &records);
    std::string shaderpacks = join_path(root, "shaderpacks");
    if (is_directory(shaderpacks)) scan_directory_tree(shaderpacks, "shaderpacks", 'S', &records);
    scan_configs(root, &records);

    // параллельный проход не блокирует UI на больших сборках
    const unsigned hardware = std::thread::hardware_concurrency();
    const unsigned workers = std::max(1u, std::min(hardware == 0 ? 4u : hardware, 16u));
    std::atomic<size_t> cursor{0};
    std::vector<std::string> lines(records.size());
    auto worker = [&]() {
        while (true) {
            size_t index = cursor.fetch_add(1);
            if (index >= records.size()) return;
            const FileRecord &record = records[index];
            switch (record.kind) {
                case 'M': {
                    ModStats stats = analyze_mod(record);
                    lines[index] = "M\t" + sanitize_tsv(record.relative) + "\t" + std::to_string(record.size) + "\t" +
                                   stats.sha256 + "\t" + sanitize_tsv(stats.loader) + "\t" + sanitize_tsv(stats.mod_id) +
                                   "\t" + sanitize_tsv(stats.name) + "\t" + sanitize_tsv(stats.version) + "\t" +
                                   std::to_string(stats.class_count) + "\t" + std::to_string(stats.unpacked_bytes);
                    break;
                }
                case 'S': {
                    PackStats stats = analyze_pack(record);
                    lines[index] = "S\t" + sanitize_tsv(record.relative) + "\t" + std::to_string(record.size) + "\t" +
                                   stats.sha256 + "\t" + std::to_string(stats.programs) + "\t" +
                                   std::to_string(stats.files);
                    break;
                }
                case 'P': {
                    uint64_t size = 0;
                    std::string digest = sha256_file(record.absolute, &size);
                    lines[index] = "P\t" + sanitize_tsv(record.relative) + "\t" + std::to_string(size) + "\t" + digest;
                    break;
                }
                default: {
                    uint64_t size = 0;
                    std::string digest = sha256_file(record.absolute, &size);
                    lines[index] = "C\t" + sanitize_tsv(record.relative) + "\t" + std::to_string(size) + "\t" + digest;
                    break;
                }
            }
        }
    };
    if (records.size() < 4) {
        worker();
    } else {
        std::vector<std::thread> pool;
        pool.reserve(workers);
        for (unsigned i = 0; i < workers; i++) pool.emplace_back(worker);
        for (std::thread &thread : pool) thread.join();
    }

    // сортировка фиксирует порядок TSV-строк
    std::vector<size_t> order(records.size());
    for (size_t i = 0; i < records.size(); i++) order[i] = i;
    std::sort(order.begin(), order.end(), [&](size_t a, size_t b) {
        if (records[a].kind != records[b].kind) return records[a].kind < records[b].kind;
        return records[a].relative < records[b].relative;
    });
    for (size_t index : order) {
        if (!lines[index].empty()) std::printf("%s\n", lines[index].c_str());
    }
    return 0;
}

}  // namespace nivora
