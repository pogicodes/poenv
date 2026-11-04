// Reads a .properties file with a real Java loader and prints the T_* keys as JSON. Used by tests/loaders.rs.
//   java Read.java java <file>                       java.util.Properties
//   java -cp 'jars/*' Read.java spring <file>        Spring Boot's application.properties loader
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Map;
import java.util.TreeMap;

public class Read {
    public static void main(String[] args) throws Exception {
        Path file = Path.of(args[1]);
        Map<String, String> got = new TreeMap<>();
        if (args[0].equals("java")) {
            java.util.Properties p = new java.util.Properties();
            try (InputStream in = Files.newInputStream(file)) {
                p.load(in);
            }
            for (String k : p.stringPropertyNames()) got.put(k, p.getProperty(k));
        } else {
            // Loaded by name so this file compiles without Spring on the classpath for the "java" mode.
            Class<?> loader = Class.forName("org.springframework.boot.env.PropertiesPropertySourceLoader");
            Class<?> resource = Class.forName("org.springframework.core.io.FileSystemResource");
            Object res = resource.getConstructor(Path.class).newInstance(file);
            Object l = loader.getConstructor().newInstance();
            java.util.List<?> sources = (java.util.List<?>) loader
                .getMethod("load", String.class, Class.forName("org.springframework.core.io.Resource"))
                .invoke(l, "t", res);
            Object source = sources.get(0);
            String[] names = (String[]) source.getClass().getMethod("getPropertyNames").invoke(source);
            for (String k : names) {
                Object v = source.getClass().getMethod("getProperty", String.class).invoke(source, k);
                got.put(k, v.toString());
            }
        }
        StringBuilder out = new StringBuilder("{");
        for (Map.Entry<String, String> e : got.entrySet()) {
            if (!e.getKey().startsWith("T_")) continue;
            if (out.length() > 1) out.append(',');
            out.append(json(e.getKey())).append(':').append(json(e.getValue()));
        }
        System.out.println(out.append('}'));
    }

    static String json(String s) {
        StringBuilder b = new StringBuilder("\"");
        for (char c : s.toCharArray()) {
            if (c == '"' || c == '\\') b.append('\\').append(c);
            else if (c < 0x20 || c > 0x7e) b.append(String.format("\\u%04x", (int) c));
            else b.append(c);
        }
        return b.append('"').toString();
    }
}
