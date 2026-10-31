// pcholki
// bzzzz
// pchol
// пчолки бзззззз
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cctype>
#include <fstream>
#include <memory>
#include <string>
#include <thread>
#include <vector>
#include <zlib.h>

#include "instance_manifest.hpp"

#if defined(__MINGW32__) && !defined(_GLIBCXX_HAS_GTHREADS)
// mingw с win32-потоками не поддерживает std::thread; используем CreateThread
#include <windows.h>
#define TURBO_WIN32_THREADS 1
#else
#define TURBO_WIN32_THREADS 0
#endif

#if defined(__x86_64__)
#define TURBO_X86 1
#include <immintrin.h>
#else
#define TURBO_X86 0
#endif

namespace {

#if TURBO_WIN32_THREADS
struct win32_task {
    void (*fn)(void *);
    void *arg;
};

inline DWORD WINAPI win32_trampoline(LPVOID raw) {
    win32_task *task = static_cast<win32_task *>(raw);
    task->fn(task->arg);
    return 0;
}
#endif

// запускает worker() в workers потоках; вызов WaitForMultipleObjects ждёт не более 64 потоков за раз
template <typename F>
void run_parallel(unsigned workers, F worker) {
#if TURBO_WIN32_THREADS
    std::vector<HANDLE> threads;
    std::vector<win32_task> tasks(workers);
    for (unsigned i = 0; i < workers; i++) {
        // поток освобождает собственную копию worker через unique_ptr
        F *copy = new F(worker);
        tasks[i].fn = [](void *raw) {
            std::unique_ptr<F> owned(static_cast<F *>(raw));
            (*owned)();
        };
        tasks[i].arg = copy;
        HANDLE thread = CreateThread(nullptr, 0, win32_trampoline, &tasks[i], 0, nullptr);
        if (thread == nullptr) {
            // при ошибке создания потока выполняем задачу здесь
            tasks[i].fn(tasks[i].arg);
            continue;
        }
        threads.push_back(thread);
    }
    for (size_t base = 0; base < threads.size(); ) {
        size_t left = threads.size() - base;
        DWORD chunk = left > 64 ? 64 : static_cast<DWORD>(left);
        WaitForMultipleObjects(chunk, threads.data() + base, TRUE, INFINITE);
        base += chunk;
    }
    for (HANDLE thread : threads) CloseHandle(thread);
#else
    std::vector<std::thread> threads;
    threads.reserve(workers);
    for (unsigned i = 0; i < workers; i++) threads.emplace_back(worker);
    for (auto &thread : threads) thread.join();
#endif
}

constexpr uint32_t K[64] = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

inline uint32_t rotr(uint32_t value, unsigned bits) { return (value >> bits) | (value << (32 - bits)); }

void sha256_compress_generic(uint32_t h[8], const unsigned char block[64]) {
    uint32_t w[64];
    for (int i = 0; i < 16; i++) {
        w[i] = (uint32_t(block[4 * i]) << 24) | (uint32_t(block[4 * i + 1]) << 16) |
               (uint32_t(block[4 * i + 2]) << 8) | uint32_t(block[4 * i + 3]);
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

#if TURBO_X86

__m128i ni_k_group(int group) {
    alignas(16) uint32_t words[4] = {K[4 * group], K[4 * group + 1], K[4 * group + 2], K[4 * group + 3]};
    return _mm_loadu_si128((const __m128i *)words);
}

void sha256_compress_ni(uint32_t h[8], const unsigned char block[64]) {
    const __m128i mask = _mm_set_epi64x(0x0c0d0e0f08090a0bULL, 0x0405060700010203ULL);

    // регистры A..H переставляются в ABEF/CDGH, затем собираются обратно
    __m128i tmp = _mm_loadu_si128((const __m128i *)&h[0]);
    __m128i state1 = _mm_loadu_si128((const __m128i *)&h[4]);
    tmp = _mm_shuffle_epi32(tmp, 0xB1);
    state1 = _mm_shuffle_epi32(state1, 0x1B);
    __m128i state0 = _mm_alignr_epi8(tmp, state1, 8);
    state1 = _mm_blend_epi16(state1, tmp, 0xF0);

    __m128i slots[4];
    slots[0] = _mm_shuffle_epi8(_mm_loadu_si128((const __m128i *)&block[0]), mask);
    slots[1] = _mm_shuffle_epi8(_mm_loadu_si128((const __m128i *)&block[16]), mask);
    slots[2] = _mm_shuffle_epi8(_mm_loadu_si128((const __m128i *)&block[32]), mask);
    slots[3] = _mm_shuffle_epi8(_mm_loadu_si128((const __m128i *)&block[48]), mask);

    for (int group = 0; group < 16; group++) {
        const int cur = group & 3;
        const int prev = (group + 3) & 3;
        const int next = (group + 1) & 3;

        __m128i msg = _mm_add_epi32(slots[cur], ni_k_group(group));
        state1 = _mm_sha256rnds2_epu32(state1, state0, msg);
        msg = _mm_shuffle_epi32(msg, 0x0E);
        state0 = _mm_sha256rnds2_epu32(state0, state1, msg);

        if (group >= 3 && group <= 14) {
            // достраиваем W группы group+1 через частичную сумму, W[i-7] и сигма1
            tmp = _mm_alignr_epi8(slots[cur], slots[prev], 4);
            slots[next] = _mm_add_epi32(slots[next], tmp);
            slots[next] = _mm_sha256msg2_epu32(slots[next], slots[cur]);
        }
        if (group >= 1 && group <= 12) {
            // частичная сумма для group+3 использует слова group-1 и сигма0
            slots[prev] = _mm_sha256msg1_epu32(slots[prev], slots[cur]);
        }
    }

    tmp = _mm_shuffle_epi32(state0, 0x1B);          /* lane order: [A,B,E,F] */
    __m128i tail = _mm_shuffle_epi32(state1, 0x1B); /* lane order: [C,D,G,H] */
    __m128i abcd = _mm_unpacklo_epi64(tmp, tail);   /* packed order: [A,B,C,D] */
    __m128i efgh = _mm_unpackhi_epi64(tmp, tail);   /* packed order: [E,F,G,H] */
    _mm_storeu_si128((__m128i *)&h[0], _mm_add_epi32(abcd, _mm_loadu_si128((const __m128i *)&h[0])));
    _mm_storeu_si128((__m128i *)&h[4], _mm_add_epi32(efgh, _mm_loadu_si128((const __m128i *)&h[4])));
}

#endif // closes TURBO_X86

using CompressFn = void (*)(uint32_t[8], const unsigned char[64]);

CompressFn pick_compress() {
#if TURBO_X86
    __builtin_cpu_init();
    if (__builtin_cpu_supports("sha") && __builtin_cpu_supports("sse4.1") && __builtin_cpu_supports("ssse3")) {
        return &sha256_compress_ni;
    }
#endif
    return &sha256_compress_generic;
}

struct Sha256State {
    uint32_t h[8];
    uint64_t length;
    unsigned buffered;
    unsigned char buffer[64];
};

class Hasher {
public:
    explicit Hasher(CompressFn compress) : compress_(compress) {
        static constexpr uint32_t INIT[8] = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                             0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
        std::memcpy(state_.h, INIT, sizeof(INIT));
        state_.length = 0;
        state_.buffered = 0;
    }

    void update(const unsigned char *data, size_t size) {
        state_.length += size;
        if (state_.buffered) {
            unsigned need = 64 - state_.buffered;
            if (size < need) {
                std::memcpy(state_.buffer + state_.buffered, data, size);
                state_.buffered += static_cast<unsigned>(size);
                return;
            }
            std::memcpy(state_.buffer + state_.buffered, data, need);
            compress_(state_.h, state_.buffer);
            data += need;
            size -= need;
            state_.buffered = 0;
        }
        while (size >= 64) {
            compress_(state_.h, data);
            data += 64;
            size -= 64;
        }
        if (size) {
            std::memcpy(state_.buffer, data, size);
            state_.buffered = static_cast<unsigned>(size);
        }
    }

    void final(unsigned char digest[32]) {
        uint64_t bits = state_.length * 8;
        unsigned char pad = 0x80;
        update(&pad, 1);
        unsigned char zero = 0;
        while (state_.buffered != 56) update(&zero, 1);
        unsigned char tail[8];
        for (int i = 0; i < 8; i++) tail[i] = static_cast<unsigned char>(bits >> (56 - 8 * i));
        update(tail, 8);
        for (int i = 0; i < 8; i++) {
            digest[4 * i] = static_cast<unsigned char>(state_.h[i] >> 24);
            digest[4 * i + 1] = static_cast<unsigned char>(state_.h[i] >> 16);
            digest[4 * i + 2] = static_cast<unsigned char>(state_.h[i] >> 8);
            digest[4 * i + 3] = static_cast<unsigned char>(state_.h[i]);
        }
    }

private:
    Sha256State state_;
    CompressFn compress_;
};

struct FileResult {
    std::string hex;
    uint64_t size = 0;
    bool ok = false;
};

std::string to_hex(const unsigned char digest[32]) {
    static const char digits[] = "0123456789abcdef";
    std::string out;
    out.reserve(64);
    for (int i = 0; i < 32; i++) {
        out.push_back(digits[digest[i] >> 4]);
        out.push_back(digits[digest[i] & 0xf]);
    }
    return out;
}

FileResult hash_one(const std::string &path, CompressFn compress) {
    FileResult result;
    std::ifstream file(path, std::ios::binary);
    if (!file) return result;
    Hasher hasher(compress);
    std::vector<char> buffer(1024 * 1024);
    uint64_t total = 0;
    while (file) {
        file.read(buffer.data(), static_cast<std::streamsize>(buffer.size()));
        std::streamsize got = file.gcount();
        if (got > 0) {
            hasher.update(reinterpret_cast<const unsigned char *>(buffer.data()), static_cast<size_t>(got));
            total += static_cast<uint64_t>(got);
        }
    }
    if (file.bad()) return result;
    unsigned char digest[32];
    hasher.final(digest);
    result.hex = to_hex(digest);
    result.size = total;
    result.ok = true;
    return result;
}

struct JarInfo {
    bool ok = false;  // zip прочитан; манифеста может не быть
    std::string loader;
    std::string mod_id;
    std::string name;
    std::string version;
    // результаты сканирования связывают записи стека ошибок с файлами модов
    std::vector<std::string> loaders;
    std::vector<std::string> mixins;
    std::vector<std::string> packages;
};

struct ManifestSpec {
    const char *name;
    const char *loader;
    char kind;  // 'f': json id/name/version, 't': toml, 'm': mcmod, 'l': litemod
};

// порядок = приоритет: fabric раньше quilt раньше forge-вариантов
const ManifestSpec MANIFESTS[] = {
    {"fabric.mod.json", "fabric", 'f'},
    {"quilt.mod.json", "quilt", 'f'},
    {"META-INF/mods.toml", "forge", 't'},
    {"META-INF/neoforge.mods.toml", "neoforge", 't'},
    {"mcmod.info", "forge", 'm'},
    {"litemod.json", "liteloader", 'l'},
};

constexpr uint32_t MAX_MANIFEST_BYTES = 256 * 1024;

uint16_t rd16(const unsigned char *p) { return uint16_t(p[0]) | uint16_t(p[1]) << 8; }
uint32_t rd32(const unsigned char *p) {
    return uint32_t(p[0]) | uint32_t(p[1]) << 8 | uint32_t(p[2]) << 16 | uint32_t(p[3]) << 24;
}

bool read_at(std::ifstream &file, uint64_t offset, size_t size, std::string &out) {
    out.assign(size, '\0');
    file.clear();
    file.seekg(std::streamoff(offset), std::ios::beg);
    if (!file) return false;
    file.read(&out[0], std::streamsize(size));
    return size_t(file.gcount()) == size;
}

bool skip_ws(const std::string &doc, size_t &cursor) {
    while (cursor < doc.size()) {
        char c = doc[cursor];
        if (c == ' ' || c == '\t' || c == '\n' || c == '\r') cursor++;
        else return true;
    }
    return false;
}

// ищет первое строковое поле; в манифестах нужные ключи предшествуют вложенным
std::string json_field(const std::string &doc, const char *key) {
    std::string needle = std::string("\"") + key + "\"";
    size_t pos = doc.find(needle);
    while (pos != std::string::npos) {
        size_t cursor = pos + needle.size();
        if (!skip_ws(doc, cursor) || doc[cursor] != ':') {
            pos = doc.find(needle, pos + needle.size());
            continue;
        }
        cursor++;
        if (!skip_ws(doc, cursor) || doc[cursor] != '"') {
            pos = doc.find(needle, pos + needle.size());
            continue;
        }
        cursor++;
        std::string value;
        while (cursor < doc.size() && doc[cursor] != '"') {
            if (doc[cursor] == '\\' && cursor + 1 < doc.size()) {
                char next = doc[cursor + 1];
                cursor += 2;
                if (next == 'n') value.push_back('\n');
                else if (next == 't') value.push_back('\t');
                else if (next == 'u' && cursor + 4 <= doc.size()) cursor += 4;
                else value.push_back(next);
            } else {
                value.push_back(doc[cursor++]);
            }
        }
        return value;
    }
    return {};
}

std::string read_quoted(const std::string &doc, size_t &cursor) {
    std::string value;
    while (cursor < doc.size() && doc[cursor] != '"') {
        if (doc[cursor] == '\\' && cursor + 1 < doc.size()) {
            char next = doc[cursor + 1];
            cursor += 2;
            if (next == 'n') value.push_back('\n');
            else if (next == 't') value.push_back('\t');
            else value.push_back(next);
        } else {
            value.push_back(doc[cursor++]);
        }
    }
    if (cursor < doc.size()) cursor++;
    return value;
}

// ищет key="value" без разбора TOML-таблиц
std::string toml_field(const std::string &doc, const char *key) {
    const size_t key_len = std::strlen(key);
    size_t pos = doc.find(key);
    while (pos != std::string::npos) {
        if (pos > 0 && (isalnum(static_cast<unsigned char>(doc[pos - 1])) || doc[pos - 1] == '_')) {
            pos = doc.find(key, pos + 1);
            continue;
        }
        size_t cursor = pos + key_len;
        if (!skip_ws(doc, cursor) || doc[cursor] != '=') {
            pos = doc.find(key, pos + 1);
            continue;
        }
        cursor++;
        if (!skip_ws(doc, cursor) || doc[cursor] != '"') {
            pos = doc.find(key, pos + 1);
            continue;
        }
        cursor++;
        return read_quoted(doc, cursor);
    }
    return {};
}

// архив ZIP использует raw deflate без заголовка zlib
std::string inflate_raw(const std::string &input, uint32_t expected) {
    if (expected == 0 || expected > MAX_MANIFEST_BYTES || input.empty()) return {};
    std::string out(expected, '\0');
    z_stream stream{};
    if (inflateInit2(&stream, -15) != Z_OK) return {};
    stream.next_in = reinterpret_cast<Bytef *>(const_cast<char *>(input.data()));
    stream.avail_in = static_cast<uInt>(input.size());
    stream.next_out = reinterpret_cast<Bytef *>(&out[0]);
    stream.avail_out = static_cast<uInt>(out.size());
    int status = inflate(&stream, Z_FINISH);
    bool complete = status == Z_STREAM_END && stream.avail_out == 0;
    inflateEnd(&stream);
    return complete ? out : std::string();
}

std::string to_lower_name(const std::string &value) {
    std::string out = value;
    for (char &c : out) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
    return out;
}

void push_unique(std::vector<std::string> &items, const std::string &value) {
    if (std::find(items.begin(), items.end(), value) == items.end()) items.push_back(value);
}

// собирает загрузчики, корневые mixin-конфиги и префиксы пакетов классов
void analyze_entry_name(const std::string &name, JarInfo &info) {
    const std::string lowered = to_lower_name(name);
    if (lowered == "fabric.mod.json") push_unique(info.loaders, "fabric");
    else if (lowered == "quilt.mod.json") push_unique(info.loaders, "quilt");
    else if (lowered == "meta-inf/neoforge.mods.toml") push_unique(info.loaders, "neoforge");
    else if (lowered == "meta-inf/mods.toml" || lowered == "mcmod.info") push_unique(info.loaders, "forge");
    if (lowered.size() > 6 && lowered.size() < 80 && lowered.find('/') == std::string::npos
        && lowered.rfind(".json") == lowered.size() - 5 && lowered.find("mixin") != std::string::npos
        && info.mixins.size() < 8) {
        push_unique(info.mixins, lowered);
    }
    if (info.packages.size() < 16 && lowered.size() > 6 && lowered.rfind(".class") == lowered.size() - 6
        && lowered.compare(0, 9, "meta-inf/") != 0) {
        // до трёх сегментов пути: com/example/mod
        size_t slashes[2] = {std::string::npos, std::string::npos};
        int seen = 0;
        for (size_t i = 0; i < lowered.size() && seen < 2; i++) {
            if (lowered[i] == '/') slashes[seen++] = i;
        }
        if (seen == 2) {
            size_t third = lowered.find('/', slashes[1] + 1);
            size_t end = (third != std::string::npos) ? third : slashes[1];
            push_unique(info.packages, lowered.substr(0, end));
        }
    }
}

JarInfo scan_one(const std::string &path) {    JarInfo info;
    std::ifstream file(path, std::ios::binary);
    if (!file) return info;
    file.seekg(0, std::ios::end);
    if (!file) return info;
    std::streamoff end = file.tellg();
    if (end < 0) return info;
    uint64_t size = static_cast<uint64_t>(end);
    if (size < 22) return info;

    uint64_t tail_size = std::min<uint64_t>(size, 66 * 1024);
    std::string tail;
    if (!read_at(file, size - tail_size, static_cast<size_t>(tail_size), tail)) return info;
    size_t eocd = std::string::npos;
    if (tail_size >= 22) {
        for (size_t i = tail_size - 22;; i--) {
            if (std::memcmp(tail.data() + i, "PK\x05\x06", 4) == 0) {
                uint16_t comment = rd16(reinterpret_cast<const unsigned char *>(tail.data()) + i + 20);
                if (i + 22 + comment <= tail_size) {
                    eocd = i;
                    break;
                }
            }
            if (i == 0) break;
        }
    }
    if (eocd == std::string::npos) return info;
    const unsigned char *e = reinterpret_cast<const unsigned char *>(tail.data() + eocd);
    uint32_t cd_size = rd32(e + 12);
    uint32_t cd_offset = rd32(e + 16);
    if (cd_size > 32u * 1024 * 1024 || cd_offset > size || cd_size > size || cd_offset + uint64_t(cd_size) > size) return info;

    std::string central;
    if (!read_at(file, cd_offset, cd_size, central)) return info;

    const ManifestSpec *best = nullptr;
    uint16_t best_method = 0;
    uint32_t best_comp = 0, best_uncomp = 0, best_local = 0;
    size_t pos = 0;
    while (pos + 46 <= central.size()) {
        const unsigned char *p = reinterpret_cast<const unsigned char *>(central.data() + pos);
        if (rd32(p) != 0x02014b50) break;
        uint16_t method = rd16(p + 10);
        uint32_t comp = rd32(p + 20);
        uint32_t uncomp = rd32(p + 24);
        uint16_t name_len = rd16(p + 28);
        uint16_t extra_len = rd16(p + 30);
        uint16_t comment_len = rd16(p + 32);
        uint32_t local_offset = rd32(p + 42);
        if (pos + 46 + size_t(name_len) > central.size()) break;
        std::string name(central, pos + 46, name_len);
        analyze_entry_name(name, info);
        for (const ManifestSpec &spec : MANIFESTS) {
            // порядок MANIFESTS задаёт приоритет
            if (name == spec.name && (!best || &spec < best)) {
                best = &spec;
                best_method = method;
                best_comp = comp;
                best_uncomp = uncomp;
                best_local = local_offset;
            }
        }
        pos += 46 + size_t(name_len) + extra_len + comment_len;
    }

    if (best && best_uncomp > 0 && best_uncomp <= MAX_MANIFEST_BYTES && best_comp <= 1024 * 1024 && uint64_t(best_local) + 30 <= size) {
        std::string header;
        if (read_at(file, best_local, 30, header) && rd32(reinterpret_cast<const unsigned char *>(header.data())) == 0x04034b50) {
            uint16_t name_len = rd16(reinterpret_cast<const unsigned char *>(header.data()) + 26);
            uint16_t extra_len = rd16(reinterpret_cast<const unsigned char *>(header.data()) + 28);
            uint64_t data_offset = uint64_t(best_local) + 30 + name_len + extra_len;
            if (data_offset + best_comp <= size) {
                std::string compressed;
                if (read_at(file, data_offset, best_comp, compressed)) {
                    std::string doc;
                    if (best_method == 0) doc = compressed;
                    else if (best_method == 8) doc = inflate_raw(compressed, best_uncomp);
                    if (!doc.empty()) {
                        info.loader = best->loader;
                        switch (best->kind) {
                            case 'f':
                                info.mod_id = json_field(doc, "id");
                                info.name = json_field(doc, "name");
                                info.version = json_field(doc, "version");
                                break;
                            case 't':
                                info.mod_id = toml_field(doc, "modId");
                                info.name = toml_field(doc, "displayName");
                                info.version = toml_field(doc, "version");
                                break;
                            case 'm':
                                info.mod_id = json_field(doc, "modid");
                                info.name = json_field(doc, "name");
                                info.version = json_field(doc, "version");
                                break;
                            case 'l':
                                info.mod_id.clear();
                                info.name = json_field(doc, "name");
                                info.version = json_field(doc, "version");
                                break;
                        }
                    }
                }
            }
        }
    }
    info.ok = true;
    return info;
}

std::string clean_field(const std::string &value) {
    std::string out;
    out.reserve(value.size());
    for (char c : value) {
        if (c == '\t' || c == '\n' || c == '\r') out.push_back(' ');
        else out.push_back(c);
    }
    return out.empty() ? "-" : out;
}


} // namespace

int main(int argc, char **argv) {
    if (argc < 2) {
        std::fprintf(stderr, "nivora-turbo sha256 FILE [FILE...]\n         nivora-turbo scan-jars FILE [FILE...]\n         nivora-turbo scan-mods FILE [FILE...]\n         nivora-turbo instance-manifest DIR\n");
        return 2;
    }
    if (std::strcmp(argv[1], "version") == 0) {
        std::printf("nivora-turbo 4\n");
        return 0;
    }
    if (std::strcmp(argv[1], "instance-manifest") == 0) {
        std::vector<std::string> args(argv + 2, argv + argc);
        return nivora::run_instance_manifest(args);
    }
    const bool scan_mode = std::strcmp(argv[1], "scan-jars") == 0;
    const bool mods_mode = std::strcmp(argv[1], "scan-mods") == 0;
    if ((!scan_mode && !mods_mode && std::strcmp(argv[1], "sha256") != 0) || argc < 3) {
        std::fprintf(stderr, "неизвестная команда: %s\n", argv[1]);
        return 2;
    }

    std::vector<std::string> paths(argv + 2, argv + argc);
    bool all_ok = true;
    std::string out;
    out.reserve(96 * paths.size());

    if (mods_mode) {

        std::vector<JarInfo> results(paths.size());
        unsigned workers = std::thread::hardware_concurrency();
        if (workers == 0) workers = 1;
        if (workers > paths.size()) workers = static_cast<unsigned>(paths.size());
        std::atomic<size_t> next{0};
        auto worker = [&]() {
            for (;;) {
                size_t index = next.fetch_add(1);
                if (index >= paths.size()) return;
                results[index] = scan_one(paths[index]);
            }
        };
        run_parallel(workers, worker);
        auto join_plus = [](const std::vector<std::string> &items) {
            std::string out;
            for (size_t i = 0; i < items.size(); i++) {
                if (i) out += '+';
                out += items[i];
            }
            return out;
        };
        for (size_t i = 0; i < paths.size(); i++) {
            const JarInfo &result = results[i];
            out += clean_field(join_plus(result.loaders));
            out += '\t';
            out += clean_field(result.mod_id);
            out += '\t';
            out += clean_field(result.name);
            out += '\t';
            out += clean_field(result.version);
            out += '\t';
            out += clean_field(join_plus(result.mixins));
            out += '\t';
            out += clean_field(join_plus(result.packages));
            out += '\t';
            out += result.ok ? '0' : '1';
            out += '\t';
            out += paths[i];
            out += '\n';
            if (!result.ok) all_ok = false;
        }
    } else if (scan_mode) {
        std::vector<JarInfo> results(paths.size());
        unsigned workers = std::thread::hardware_concurrency();
        if (workers == 0) workers = 1;
        if (workers > paths.size()) workers = static_cast<unsigned>(paths.size());
        std::atomic<size_t> next{0};
        auto worker = [&]() {
            for (;;) {
                size_t index = next.fetch_add(1);
                if (index >= paths.size()) return;
                results[index] = scan_one(paths[index]);
            }
        };
        run_parallel(workers, worker);
        for (size_t i = 0; i < paths.size(); i++) {
            if (results[i].ok) {
                out += clean_field(results[i].loader);
                out += '\t';
                out += clean_field(results[i].mod_id);
                out += '\t';
                out += clean_field(results[i].name);
                out += '\t';
                out += clean_field(results[i].version);
                out += '\t';
                out += paths[i];
                out += '\n';
            } else {
                out += "-\t-\t-\t-\t";
                out += paths[i];
                out += '\n';
                all_ok = false;
            }
        }
    } else {gh
        std::vector<FileResult> results(paths.size());
        CompressFn compress = pick_compress();
        unsigned workers = std::thread::hardware_concurrency();
        if (workers == 0) workers = 1;
        if (workers > paths.size()) workers = static_cast<unsigned>(paths.size());
        std::atomic<size_t> next{0};
        auto worker = [&]() {
            for (;;) {
                size_t index = next.fetch_add(1);
                if (index >= paths.size()) return;
                results[index] = hash_one(paths[index], compress);
            }
        };
        run_parallel(workers, worker);
        for (size_t i = 0; i < paths.size(); i++) {
            if (results[i].ok) {
                out += results[i].hex;
                out += '\t';
                out += std::to_string(results[i].size);
                out += '\t';
                out += paths[i];
                out += '\n';
            } else {
                out += "-\t-\t";
                out += paths[i];
                out += '\n';
                all_ok = false;
            }
        }
    }

    std::fwrite(out.data(), 1, out.size(), stdout);
    return all_ok ? 0 : 1;
}
