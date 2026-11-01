package io.github.skunkwerkx.hypertabular.probe;

import io.github.skunkwerkx.hypertabular.Batch;
import io.github.skunkwerkx.hypertabular.Column;
import io.github.skunkwerkx.hypertabular.DelimitedReader;
import io.github.skunkwerkx.hypertabular.Dialect;
import io.github.skunkwerkx.hypertabular.Tabular;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.util.List;

/**
 * The module-path child's program. It lives in its own package: the library's package belongs to
 * the automatic module there, so a class of this package name on the class path would be hidden.
 * The backend is read reflectively (an automatic module opens all its packages).
 */
public final class ModuleMain {
    private ModuleMain() {}

    public static void main(String[] args) throws Exception {
        byte[] csv = "n\n42\n".getBytes(StandardCharsets.UTF_8);
        try (DelimitedReader reader = DelimitedReader.of(csv, Dialect.CSV, List.of(Column.i32(0)))) {
            Batch batch = reader.read();
            System.out.println("cell: " + batch.get(0, 0, Integer.class));
        }
        Method backend =
                Class.forName("io.github.skunkwerkx.hypertabular.Native").getDeclaredMethod("backend");
        backend.setAccessible(true);
        System.out.println("backend: " + backend.invoke(null));
        System.out.println("module: " + Tabular.class.getModule().getName());
    }
}
