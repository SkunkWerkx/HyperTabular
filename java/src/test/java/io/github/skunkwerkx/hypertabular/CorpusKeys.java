package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertTrue;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.io.IOException;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Consumer;
import java.util.function.Function;

/**
 * The keys a replay reads, at every level of both corpus files: a replay fails on any other, so
 * that a key added to a corpus cannot be passed over. A workbook case's {@code note} is the one
 * key no replay reads, a sentence for whoever reads the file.
 */
final class CorpusKeys {
    private CorpusKeys() {}

    private static final Set<String> FORMAT = Set.of("decimal_sep", "group_sep", "flags", "currency");
    private static final Set<String> PLAN =
            Set.of("door", "ordinal", "precision", "order", "epoch", "format", "tokens", "options", "constraints");
    private static final Set<String> CONSTRAINTS = Set.of(
            "minimum",
            "exclusiveMinimum",
            "maximum",
            "exclusiveMaximum",
            "multipleOf",
            "rounding",
            "minLength",
            "maxLength",
            "enum",
            "trim",
            "caseSensitive");
    private static final Set<String> DATE_OPTIONS = Set.of("flags", "pivot", "months");
    private static final Set<String> CELL = Set.of(
            "expect",
            "fault",
            "raw",
            "raw_hex",
            "detail",
            "origin",
            "value",
            "text",
            "magnitude",
            "negative",
            "scale",
            "seconds",
            "nanos",
            "year",
            "month",
            "day",
            "nanos_of_day",
            "keyword",
            "adjusted",
            "enum_index",
            "bits");
    private static final Set<String> PROBLEM = Set.of(
            "row",
            "line",
            "plan_column",
            "source_column",
            "reason",
            "detail",
            "origin",
            "fault",
            "expected",
            "found",
            "keyword");
    private static final Set<String> FAILURE = Set.of("kind", "record", "line", "byte", "expected", "found");
    private static final Set<String> SUGGEST = Set.of("columns", "truncated", "sample_rows", "format");
    private static final Set<String> SUGGESTED = Set.of("door", "param", "status", "matched", "sampled");

    private static final Set<String> DELIMITED =
            Set.of("name", "input", "input_hex", "dialect", "plan", "header", "rows", "problems", "failure", "suggest");
    private static final Set<String> DIALECT = Set.of(
            "separator",
            "quoting",
            "has_header",
            "skip_blank_lines",
            "skip",
            "header_row",
            "short_rows",
            "long_rows",
            "tokens",
            "encoding");

    private static final Set<String> WORKBOOK = Set.of(
            "name",
            "note",
            "file",
            "format",
            "epoch",
            "sheets",
            "sheet",
            "options",
            "plan",
            "header",
            "rows",
            "numbers",
            "problems",
            "failure",
            "suggest");
    private static final Set<String> SHEET = Set.of("name", "hidden");
    private static final Set<String> SHEET_OPTIONS =
            Set.of("has_header", "skip_empty_rows", "skip", "header_row", "range", "tokens");

    static List<String> unknownDelimited(JsonArray cases) {
        List<String> unknown = new ArrayList<>();
        for (JsonElement element : cases) {
            JsonObject vector = element.getAsJsonObject();
            String name = nameOf(vector);
            check(vector, name, DELIMITED, unknown);
            check(vector.getAsJsonObject("dialect"), name + ".dialect", DIALECT, unknown);
            checkShared(vector, name, unknown);
        }
        return unknown;
    }

    static List<String> unknownWorkbook(JsonArray cases) {
        List<String> unknown = new ArrayList<>();
        for (JsonElement element : cases) {
            JsonObject vector = element.getAsJsonObject();
            String name = nameOf(vector);
            check(vector, name, WORKBOOK, unknown);
            if (vector.has("options")) {
                check(vector.getAsJsonObject("options"), name + ".options", SHEET_OPTIONS, unknown);
            }
            if (vector.has("sheets")) {
                for (JsonElement sheet : vector.getAsJsonArray("sheets")) {
                    check(sheet.getAsJsonObject(), name + ".sheets[]", SHEET, unknown);
                }
            }
            checkShared(vector, name, unknown);
        }
        return unknown;
    }

    private static String nameOf(JsonObject vector) {
        return vector.has("name") ? vector.get("name").getAsString() : "a case with no name";
    }

    private static void checkShared(JsonObject vector, String name, List<String> unknown) {
        if (vector.has("plan")) {
            for (JsonElement element : vector.getAsJsonArray("plan")) {
                JsonObject entry = element.getAsJsonObject();
                check(entry, name + ".plan[]", PLAN, unknown);
                if (entry.has("format")) {
                    check(entry.getAsJsonObject("format"), name + ".plan[].format", FORMAT, unknown);
                }
                if (entry.has("options")) {
                    check(entry.getAsJsonObject("options"), name + ".plan[].options", DATE_OPTIONS, unknown);
                }
                if (entry.has("constraints")) {
                    check(entry.getAsJsonObject("constraints"), name + ".plan[].constraints", CONSTRAINTS, unknown);
                }
            }
        }
        if (vector.has("rows")) {
            for (JsonElement row : vector.getAsJsonArray("rows")) {
                for (JsonElement cell : row.getAsJsonArray()) {
                    check(cell.getAsJsonObject(), name + ".rows[][]", CELL, unknown);
                }
            }
        }
        if (vector.has("problems")) {
            for (JsonElement problem : vector.getAsJsonArray("problems")) {
                check(problem.getAsJsonObject(), name + ".problems[]", PROBLEM, unknown);
            }
        }
        if (vector.has("failure") && vector.get("failure").isJsonObject()) {
            check(vector.getAsJsonObject("failure"), name + ".failure", FAILURE, unknown);
        }
        if (vector.has("suggest")) {
            JsonObject suggest = vector.getAsJsonObject("suggest");
            check(suggest, name + ".suggest", SUGGEST, unknown);
            if (suggest.has("format")) {
                check(suggest.getAsJsonObject("format"), name + ".suggest.format", FORMAT, unknown);
            }
            for (JsonElement column : suggest.getAsJsonArray("columns")) {
                check(column.getAsJsonObject(), name + ".suggest.columns[]", SUGGESTED, unknown);
            }
        }
    }

    private static void check(JsonObject entry, String path, Set<String> known, List<String> unknown) {
        for (String key : entry.keySet()) {
            if (!known.contains(key)) {
                unknown.add(path + "." + key);
            }
        }
    }

    /**
     * A made-up key put in each place the file holds an object, once per kind of place, has to be
     * reported; the file with none of them has to report nothing.
     */
    static void aJunkKeyIsSeenEverywhere(String file, Function<JsonArray, List<String>> unknown) throws IOException {
        JsonArray cases = JsonParser.parseString(Files.readString(CorpusTest.CORPUS_DIRECTORY.resolve(file)))
                .getAsJsonArray();
        assertEquals(List.of(), unknown.apply(cases));
        Set<String> places = new HashSet<>();
        for (JsonElement vector : cases) {
            junk(
                    vector,
                    "",
                    entry -> {
                        entry.addProperty("junk_key", 1);
                        JsonArray one = new JsonArray();
                        one.add(vector);
                        List<String> found = unknown.apply(one);
                        entry.remove("junk_key");
                        assertEquals(1, found.size(), found.toString());
                        assertTrue(found.get(0).endsWith(".junk_key"), found.get(0));
                    },
                    places);
        }
        assertTrue(places.size() > 5, places.toString());
    }

    private static void junk(JsonElement node, String place, Consumer<JsonObject> visit, Set<String> places) {
        if (node.isJsonObject()) {
            JsonObject entry = node.getAsJsonObject();
            if (places.add(place)) {
                visit.accept(entry);
            }
            for (Map.Entry<String, JsonElement> pair : new ArrayList<>(entry.entrySet())) {
                junk(pair.getValue(), place + "." + pair.getKey(), visit, places);
            }
        } else if (node.isJsonArray()) {
            for (JsonElement value : node.getAsJsonArray()) {
                junk(value, place + "[]", visit, places);
            }
        }
    }
}
