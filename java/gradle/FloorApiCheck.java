import java.io.DataInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Deque;
import java.util.HashSet;
import java.util.List;
import java.util.Set;
import java.util.TreeSet;
import java.util.regex.Matcher;
import java.util.regex.Pattern;
import java.util.stream.Stream;

/**
 * The floor-API check the build runs on the floor JDK ({@code checkFloorApi} in
 * build.gradle.kts): every JDK class, method and field the given class files reference must
 * exist in the running JDK with the same descriptor. The base compiles {@code -source/-target
 * 21} against JDK 25's API rather than {@code --release 21} (java.lang.foreign is preview in
 * 21's signatures, and the public API carries it), so this is the half of {@code --release}
 * that would otherwise be lost. Run as a source file: {@code java FloorApiCheck.java <dir>...}.
 */
public final class FloorApiCheck {
    private static final Pattern CLASS_IN_DESCRIPTOR = Pattern.compile("L([^;<]+);");

    public static void main(String[] args) throws IOException {
        Set<String> failures = new TreeSet<>();
        int[] refs = {0};
        for (String dir : args) {
            try (Stream<Path> files = Files.walk(Path.of(dir))) {
                for (Path file : (Iterable<Path>) files.filter(p -> p.toString().endsWith(".class"))::iterator) {
                    try (InputStream in = Files.newInputStream(file)) {
                        check(new DataInputStream(in), file, failures, refs);
                    }
                }
            }
        }
        if (!failures.isEmpty()) {
            System.err.println("References to JDK API missing from JDK "
                    + Runtime.version().feature() + ":");
            failures.forEach(f -> System.err.println("  " + f));
            System.exit(1);
        }
        System.out.println("floor API: " + refs[0] + " JDK references resolve on JDK "
                + Runtime.version().feature());
    }

    private static void check(DataInputStream in, Path file, Set<String> failures, int[] refs) throws IOException {
        if (in.readInt() != 0xCAFEBABE) {
            throw new IOException(file + " is not a class file");
        }
        in.readUnsignedShort();
        in.readUnsignedShort();
        int count = in.readUnsignedShort();
        int[] tag = new int[count];
        int[] a = new int[count];
        int[] b = new int[count];
        String[] utf8 = new String[count];
        for (int i = 1; i < count; i++) {
            tag[i] = in.readUnsignedByte();
            switch (tag[i]) {
                case 1 -> utf8[i] = in.readUTF();
                case 3, 4 -> in.readInt();
                case 5, 6 -> {
                    in.readLong();
                    i++;
                }
                case 7, 8, 16, 19, 20 -> a[i] = in.readUnsignedShort();
                case 9, 10, 11, 12, 17, 18 -> {
                    a[i] = in.readUnsignedShort();
                    b[i] = in.readUnsignedShort();
                }
                case 15 -> {
                    in.readUnsignedByte();
                    a[i] = in.readUnsignedShort();
                }
                default -> throw new IOException(file + ": constant pool tag " + tag[i]);
            }
        }
        String where = file.getFileName().toString();
        List<String> descriptors = new ArrayList<>();
        for (int i = 1; i < count; i++) {
            switch (tag[i]) {
                case 7 -> {
                    String name = utf8[a[i]];
                    if (name.startsWith("[")) {
                        descriptors.add(name);
                    } else {
                        type(name, where, failures, refs);
                    }
                }
                case 9, 10, 11 -> {
                    String owner = utf8[a[a[i]]];
                    String name = utf8[a[b[i]]];
                    String descriptor = utf8[b[b[i]]];
                    descriptors.add(descriptor);
                    if (jdk(owner)) {
                        refs[0]++;
                        Class<?> type = load(owner);
                        boolean found = type == null
                                || (tag[i] == 9 ? hasField(type, name, descriptor) : hasMethod(type, name, descriptor));
                        if (!found) {
                            failures.add(where + ": " + owner + "." + name + descriptor);
                        }
                    }
                }
                case 12 -> descriptors.add(utf8[b[i]]);
                case 16 -> descriptors.add(utf8[a[i]]);
                default -> {}
            }
        }
        in.readUnsignedShort();
        in.readUnsignedShort();
        in.readUnsignedShort();
        for (int n = in.readUnsignedShort(); n > 0; n--) {
            in.readUnsignedShort();
        }
        for (int members = 0; members < 2; members++) {
            for (int n = in.readUnsignedShort(); n > 0; n--) {
                in.readUnsignedShort();
                in.readUnsignedShort();
                descriptors.add(utf8[in.readUnsignedShort()]);
                skipAttributes(in);
            }
        }
        for (String descriptor : descriptors) {
            Matcher m = CLASS_IN_DESCRIPTOR.matcher(descriptor);
            while (m.find()) {
                type(m.group(1), where, failures, refs);
            }
        }
    }

    private static void skipAttributes(DataInputStream in) throws IOException {
        for (int n = in.readUnsignedShort(); n > 0; n--) {
            in.readUnsignedShort();
            in.skipNBytes(in.readInt() & 0xFFFFFFFFL);
        }
    }

    private static boolean jdk(String internalName) {
        return internalName.startsWith("java/") || internalName.startsWith("javax/") || internalName.startsWith("jdk/");
    }

    private static void type(String internalName, String where, Set<String> failures, int[] refs) {
        if (jdk(internalName)) {
            refs[0]++;
            if (load(internalName) == null) {
                failures.add(where + ": class " + internalName);
            }
        }
    }

    private static Class<?> load(String internalName) {
        try {
            return Class.forName(internalName.replace('/', '.'), false, ClassLoader.getSystemClassLoader());
        } catch (ClassNotFoundException missing) {
            return null;
        }
    }

    private static boolean hasField(Class<?> type, String name, String descriptor) {
        for (Class<?> c : hierarchy(type)) {
            for (Field f : c.getDeclaredFields()) {
                if (f.getName().equals(name) && descriptor(f.getType()).equals(descriptor)) {
                    return true;
                }
            }
        }
        return false;
    }

    private static boolean hasMethod(Class<?> type, String name, String descriptor) {
        if (name.equals("<init>")) {
            for (Constructor<?> c : type.getDeclaredConstructors()) {
                if (descriptor(c.getParameterTypes(), void.class).equals(descriptor)) {
                    return true;
                }
            }
            return false;
        }
        if (type.isArray() || name.equals("<clinit>")) {
            return true;
        }
        for (Class<?> c : hierarchy(type)) {
            for (Method m : c.getDeclaredMethods()) {
                if (!m.getName().equals(name)) {
                    continue;
                }
                // A signature-polymorphic method (MethodHandle.invokeExact, VarHandle.get):
                // the call site's descriptor is its own.
                boolean polymorphic =
                        (c == java.lang.invoke.MethodHandle.class || c == java.lang.invoke.VarHandle.class)
                                && Modifier.isNative(m.getModifiers())
                                && m.isVarArgs();
                if (polymorphic
                        || descriptor(m.getParameterTypes(), m.getReturnType()).equals(descriptor)) {
                    return true;
                }
            }
        }
        return false;
    }

    private static List<Class<?>> hierarchy(Class<?> type) {
        List<Class<?>> out = new ArrayList<>();
        Set<Class<?>> seen = new HashSet<>();
        Deque<Class<?>> queue = new ArrayDeque<>();
        queue.add(type);
        while (!queue.isEmpty()) {
            Class<?> c = queue.poll();
            if (!seen.add(c)) {
                continue;
            }
            out.add(c);
            if (c.getSuperclass() != null) {
                queue.add(c.getSuperclass());
            }
            queue.addAll(List.of(c.getInterfaces()));
        }
        if (type.isInterface()) {
            out.add(Object.class);
        }
        return out;
    }

    private static String descriptor(Class<?>[] parameters, Class<?> result) {
        StringBuilder sb = new StringBuilder("(");
        for (Class<?> p : parameters) {
            sb.append(descriptor(p));
        }
        return sb.append(')').append(descriptor(result)).toString();
    }

    private static String descriptor(Class<?> type) {
        return type.descriptorString();
    }
}
