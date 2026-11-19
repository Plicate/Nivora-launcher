// сканирует метаданные модов и версии Java по JAR-файлам
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

public class ModScan {

    private static final int MAX_MANIFEST_BYTES = 256 * 1024;
    private static final int MAX_CLASS_ENTRIES = 4096;

    // Java 8: record недоступен, поэтому обычный неизменяемый класс
    static final class Manifest {
        private final String loader;
        private final String kind;

        Manifest(String loader, String kind) {
            this.loader = loader;
            this.kind = kind;
        }

        String loader() {
            return loader;
        }

        String kind() {
            return kind;
        }
    }

    static final Manifest[] MANIFESTS = {
        new Manifest("fabric.mod.json", "fabric"),
        new Manifest("quilt.mod.json", "quilt"),
        new Manifest("META-INF/mods.toml", "forge"),
        new Manifest("META-INF/neoforge.mods.toml", "neoforge"),
        new Manifest("mcmod.info", "mcmod"),
        new Manifest("litemod.json", "lite"),
    };

    public static void main(String[] args) {
        if (args.length == 0) {
            System.err.println("ModScan <jar> [jar...]");
            System.exit(2);
        }
        StringBuilder out = new StringBuilder();
        for (String arg : args) {
            out.append(scan(arg)).append('\n');
        }
        System.out.print(out);
    }

    static String scan(String path) {
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("path", path);
        result.put("loader", "");
        result.put("mod_id", "");
        result.put("name", "");
        result.put("version", "");
        result.put("class_major", 0);
        result.put("java_major", 0);
        result.put("dependencies", new ArrayList<>());
        result.put("breaks", new ArrayList<>());
        Path file = Paths.get(path);
        if (!Files.isRegularFile(file)) {
            return Json.write(result);
        }
        try (ZipFile zip = new ZipFile(file.toFile())) {
            Manifest found = null;
            String doc = null;
            for (Manifest manifest : MANIFESTS) {
                ZipEntry entry = zip.getEntry(manifest.loader());
                if (entry == null || entry.isDirectory() || entry.getSize() > MAX_MANIFEST_BYTES) {
                    continue;
                }
                doc = readEntry(zip, entry);
                if (doc != null) {
                    found = manifest;
                    break;
                }
            }
            int maxMajor = scanClassMajors(zip);
            result.put("class_major", maxMajor);
            result.put("java_major", majorToJava(maxMajor));
            if (found != null && doc != null) {
                applyManifest(result, found.kind(), doc);
            }
        } catch (IOException | RuntimeException malformed) {
            // повреждённый JAR возвращается с начальными значениями
        }
        return Json.write(result);
    }

    // аналог InputStream.readNBytes для Java 8 (полное чтение до limit байт)
    static byte[] readBounded(InputStream stream, int limit) throws IOException {
        java.io.ByteArrayOutputStream buffer = new java.io.ByteArrayOutputStream(Math.min(limit, 8192));
        byte[] chunk = new byte[4096];
        int total = 0;
        while (total < limit) {
            int want = Math.min(chunk.length, limit - total);
            int read = stream.read(chunk, 0, want);
            if (read < 0) {
                break;
            }
            buffer.write(chunk, 0, read);
            total += read;
        }
        return buffer.toByteArray();
    }

    static String readEntry(ZipFile zip, ZipEntry entry) throws IOException {
        try (InputStream stream = zip.getInputStream(entry)) {
            byte[] raw = readBounded(stream, MAX_MANIFEST_BYTES + 1);
            if (raw.length > MAX_MANIFEST_BYTES) {
                return null;
            }
            return new String(raw, StandardCharsets.UTF_8);
        }
    }

    // минимальную Java определяет максимальная версия class-файла
    static int scanClassMajors(ZipFile zip) {
        int maxMajor = 0;
        int seen = 0;
        java.util.Enumeration<? extends ZipEntry> entries = zip.entries();
        while (entries.hasMoreElements() && seen < MAX_CLASS_ENTRIES) {
            ZipEntry entry = entries.nextElement();
            String name = entry.getName();
            if (entry.isDirectory() || !name.endsWith(".class")) {
                continue;
            }
            seen++;
            try (InputStream stream = zip.getInputStream(entry)) {
                byte[] header = readBounded(stream, 8);
                if (header.length == 8
                        && (header[0] & 0xff) == 0xca && (header[1] & 0xff) == 0xfe
                        && (header[2] & 0xff) == 0xba && (header[3] & 0xff) == 0xbe) {
                    int major = ((header[6] & 0xff) << 8) | (header[7] & 0xff);
                    if (major > maxMajor) {
                        maxMajor = major;
                    }
                }
            } catch (IOException ignored) {
                // пропускаем повреждённый class, не прерывая общий вывод
            }
        }
        return maxMajor;
    }

    static int majorToJava(int major) {
        if (major <= 0) {
            return 0;
        }
        return Math.max(8, major - 44);
    }

    static void applyManifest(Map<String, Object> result, String kind, String doc) {
        switch (kind) {
            case "fabric":
                applyFabric(result, doc, false);
                break;
            case "quilt":
                applyFabric(result, doc, true);
                break;
            case "forge":
                applyToml(result, doc);
                break;
            case "mcmod":
                applyMcmod(result, doc);
                break;
            case "lite":
                applyLite(result, doc);
                break;
            default:
                break;
        }
    }

    @SuppressWarnings("unchecked")
    static void applyFabric(Map<String, Object> result, String doc, boolean quilt) {
        Object parsed = Json.parse(doc);
        Map<String, Object> root = (Map<String, Object>) parsed;
        if (root == null) {
            return;
        }
        Map<String, Object> loader = quilt ? asMap(root.get("quilt_loader")) : root;
        if (loader == null) {
            return;
        }
        result.put("loader", quilt ? "quilt" : "fabric");
        result.put("mod_id", text(loader.get("id")));
        result.put("version", text(loader.get("version")));
        // quilt хранит имя в quilt_loader.metadata.name, fabric — в корне "name"
        Object nameValue;
        if (quilt) {
            Map<String, Object> metadata = asMap(loader.get("metadata"));
            nameValue = metadata == null ? null : metadata.get("name");
            if (nameValue == null) {
                nameValue = root.get("name");
            }
        } else {
            nameValue = loader.get("name");
        }
        result.put("name", text(nameValue));
        List<Object> dependencies = (List<Object>) result.get("dependencies");
        collectDependencies(dependencies, asMap(loader.get("depends")), false);
        collectDependencies(dependencies, asMap(loader.get("suggests")), true);
        List<Object> breaks = (List<Object>) result.get("breaks");
        Map<String, Object> breaksMap = asMap(loader.get("breaks"));
        if (breaksMap != null) {
            breaks.addAll(breaksMap.keySet());
        }
    }

    static void collectDependencies(List<Object> output, Map<String, Object> mapping, boolean optional) {
        if (mapping == null) {
            return;
        }
        for (Map.Entry<String, Object> entry : mapping.entrySet()) {
            Map<String, Object> item = new LinkedHashMap<>();
            item.put("id", entry.getKey());
            item.put("constraint", entry.getValue() == null ? "любая" : String.valueOf(entry.getValue()));
            item.put("optional", optional);
            output.add(item);
        }
    }

    static void applyToml(Map<String, Object> result, String doc) {
        String modId = tomlField(doc, "modId");
        if (modId == null) {
            return;
        }
        result.put("loader", doc.contains("neoforge") ? "neoforge" : "forge");
        result.put("mod_id", modId);
        result.put("name", orEmpty(tomlField(doc, "displayName"), modId));
        result.put("version", orEmpty(tomlField(doc, "version"), ""));
    }

    static void applyMcmod(Map<String, Object> result, String doc) {
        Object parsed = Json.parse(doc);
        List<Object> list = (List<Object>) parsed;
        Map<String, Object> first = list == null || list.isEmpty() ? asMap(parsed) : asMap(list.get(0));
        if (first == null) {
            return;
        }
        result.put("loader", "forge");
        result.put("mod_id", text(first.get("modid")));
        result.put("name", text(first.get("name")));
        result.put("version", text(first.get("version")));
    }

    static void applyLite(Map<String, Object> result, String doc) {
        Object parsed = Json.parse(doc);
        Map<String, Object> root = (Map<String, Object>) parsed;
        if (root == null) {
            return;
        }
        result.put("loader", "liteloader");
        result.put("name", text(root.get("name")));
        result.put("version", text(root.get("version")));
    }

    static String orEmpty(String value, String fallback) {
        return value == null || value.trim().isEmpty() ? fallback : value;
    }

    static String text(Object value) {
        return value == null ? "" : String.valueOf(value);
    }

    @SuppressWarnings("unchecked")
    static Map<String, Object> asMap(Object value) {
        if (value instanceof Map<?, ?>) {
            Map<?, ?> map = (Map<?, ?>) value;
            return (Map<String, Object>) map;
        }
        return null;
    }

    static String tomlField(String doc, String key) {
        int pos = doc.indexOf(key);
        while (pos >= 0) {
            boolean wordStart = pos == 0
                    || (!Character.isLetterOrDigit(doc.charAt(pos - 1)) && doc.charAt(pos - 1) != '_');
            if (!wordStart) {
                pos = doc.indexOf(key, pos + key.length());
                continue;
            }
            int cursor = skipSpace(doc, pos + key.length());
            if (cursor >= doc.length() || doc.charAt(cursor) != '=') {
                pos = doc.indexOf(key, pos + key.length());
                continue;
            }
            cursor = skipSpace(doc, cursor + 1);
            if (cursor < doc.length() && doc.charAt(cursor) == '"') {
                return readQuoted(doc, cursor + 1);
            }
            pos = doc.indexOf(key, pos + key.length());
        }
        return null;
    }

    static int skipSpace(String doc, int index) {
        while (index < doc.length() && Character.isWhitespace(doc.charAt(index))) {
            index++;
        }
        return index;
    }

    static String readQuoted(String doc, int start) {
        StringBuilder value = new StringBuilder();
        int cursor = start;
        while (cursor < doc.length() && doc.charAt(cursor) != '"') {
            if (doc.charAt(cursor) == '\\' && cursor + 1 < doc.length()) {
                cursor++;
                char next = doc.charAt(cursor++);
                if (next == 'n') {
                    value.append('\n');
                } else if (next == 't') {
                    value.append('\t');
                } else if (next == 'r') {
                    value.append('\r');
                } else {
                    value.append(next);
                }
            } else {
                value.append(doc.charAt(cursor++));
            }
        }
        return value.toString();
    }

    /** разбор без зависимостей: Map/List/String/Double/Boolean/null */
    static final class Json {
        private final String text;
        private int index;

        private Json(String text) {
            this.text = text;
        }

        static Object parse(String source) {
            if (source == null || source.trim().isEmpty()) {
                return null;
            }
            try {
                Json parser = new Json(source);
                parser.skip();
                return parser.value();
            } catch (RuntimeException malformed) {
                return null;
            }
        }

        static String write(Map<String, Object> value) {
            StringBuilder out = new StringBuilder(256);
            writeValue(out, value);
            return out.toString();
        }

        private static void writeValue(StringBuilder out, Object value) {
            if (value == null) {
                out.append("null");
            } else if (value instanceof String) {
                writeString(out, (String) value);
            } else if (value instanceof Double || value instanceof Integer || value instanceof Long) {
                out.append(value);
            } else if (value instanceof Boolean) {
                out.append(value);
            } else if (value instanceof List<?>) {
                List<?> list = (List<?>) value;
                out.append('[');
                for (int i = 0; i < list.size(); i++) {
                    if (i > 0) {
                        out.append(',');
                    }
                    writeValue(out, list.get(i));
                }
                out.append(']');
            } else if (value instanceof Map<?, ?>) {
                Map<?, ?> map = (Map<?, ?>) value;
                out.append('{');
                boolean first = true;
                for (Map.Entry<?, ?> entry : map.entrySet()) {
                    if (!first) {
                        out.append(',');
                    }
                    first = false;
                    writeString(out, String.valueOf(entry.getKey()));
                    out.append(':');
                    writeValue(out, entry.getValue());
                }
                out.append('}');
            } else {
                writeString(out, String.valueOf(value));
            }
        }

        private static void writeString(StringBuilder out, String value) {
            out.append('"');
            for (int i = 0; i < value.length(); i++) {
                char c = value.charAt(i);
                switch (c) {
                    case '"':
                        out.append("\\\"");
                        break;
                    case '\\':
                        out.append("\\\\");
                        break;
                    case '\n':
                        out.append("\\n");
                        break;
                    case '\t':
                        out.append("\\t");
                        break;
                    case '\r':
                        out.append("\\r");
                        break;
                    default: {
                        if (c < 0x20) {
                            out.append(String.format("\\u%04x", (int) c));
                        } else {
                            out.append(c);
                        }
                    }
                }
            }
            out.append('"');
        }

        private Object value() {
            char c = text.charAt(index);
            switch (c) {
                case '{':
                    return object();
                case '[':
                    return array();
                case '"':
                    return readString();
                case 't':
                    return literal("true", Boolean.TRUE);
                case 'f':
                    return literal("false", Boolean.FALSE);
                case 'n':
                    return literal("null", null);
                default:
                    return number();
            }
        }

        private Object literal(String word, Object value) {
            if (!text.startsWith(word, index)) {
                throw new IllegalArgumentException("bad literal");
            }
            index += word.length();
            skip();
            return value;
        }

        private Map<String, Object> object() {
            Map<String, Object> out = new LinkedHashMap<>();
            index++;
            skip();
            if (index < text.length() && text.charAt(index) == '}') {
                index++;
                return out;
            }
            while (index < text.length()) {
                String key = readString();
                skip();
                if (text.charAt(index) != ':') {
                    throw new IllegalArgumentException("expected ':'");
                }
                index++;
                skip();
                out.put(key, value());
                skip();
                if (index >= text.length()) {
                    break;
                }
                if (text.charAt(index) == ',') {
                    index++;
                    skip();
                    continue;
                }
                if (text.charAt(index) == '}') {
                    index++;
                    break;
                }
                throw new IllegalArgumentException("expected ',' or '}'");
            }
            return out;
        }

        private List<Object> array() {
            List<Object> out = new ArrayList<>();
            index++;
            skip();
            if (index < text.length() && text.charAt(index) == ']') {
                index++;
                return out;
            }
            while (index < text.length()) {
                out.add(value());
                skip();
                if (index >= text.length()) {
                    break;
                }
                if (text.charAt(index) == ',') {
                    index++;
                    skip();
                    continue;
                }
                if (text.charAt(index) == ']') {
                    index++;
                    break;
                }
                throw new IllegalArgumentException("expected ',' or ']'");
            }
            return out;
        }

        private String readString() {
            if (text.charAt(index) != '"') {
                throw new IllegalArgumentException("expected string");
            }
            index++;
            StringBuilder out = new StringBuilder();
            while (index < text.length() && text.charAt(index) != '"') {
                char c = text.charAt(index++);
                if (c == '\\' && index < text.length()) {
                    char next = text.charAt(index++);
                    char decoded;
                    if (next == 'n') {
                        decoded = '\n';
                    } else if (next == 't') {
                        decoded = '\t';
                    } else if (next == 'r') {
                        decoded = '\r';
                    } else if (next == 'b') {
                        decoded = '\b';
                    } else if (next == 'f') {
                        decoded = '\f';
                    } else if (next == 'u') {
                        if (index + 4 > text.length()) {
                            throw new IllegalArgumentException("bad \\u");
                        }
                        decoded = (char) Integer.parseInt(text.substring(index, index + 4), 16);
                        index += 4;
                    } else {
                        decoded = next;
                    }
                    out.append(decoded);
                } else {
                    out.append(c);
                }
            }
            index++;
            skip();
            return out.toString();
        }

        private Object number() {
            int start = index;
            while (index < text.length() && "+-0123456789.eE".indexOf(text.charAt(index)) >= 0) {
                index++;
            }
            Double value = Double.parseDouble(text.substring(start, index));
            skip();
            return value;
        }

        private void skip() {
            while (index < text.length() && Character.isWhitespace(text.charAt(index))) {
                index++;
            }
            // mcmod.info может содержать однострочные комментарии
            if (index + 1 < text.length() && text.charAt(index) == '/' && text.charAt(index + 1) == '/') {
                while (index < text.length() && text.charAt(index) != '\n') {
                    index++;
                }
                skip();
            }
        }
    }
}
