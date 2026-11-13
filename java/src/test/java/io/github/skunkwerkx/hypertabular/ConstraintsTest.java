package io.github.skunkwerkx.hypertabular;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import io.github.skunkwerkx.hypercast.CastFailure;
import io.github.skunkwerkx.hypercast.Checked;
import io.github.skunkwerkx.hypercast.Constraints;
import io.github.skunkwerkx.hypercast.Fault;
import io.github.skunkwerkx.hypercast.Keyword;
import io.github.skunkwerkx.hypercast.Rounding;
import io.github.skunkwerkx.hypercast.Violation;
import java.lang.foreign.Arena;
import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.OptionalInt;
import org.junit.jupiter.api.Test;

/** A column that declares HyperCast's constraints: what the corpus does not reach of the binding's surface. */
class ConstraintsTest {
    private static final Dialect HEADERLESS = Dialect.CSV.withHeader(false);

    private static DelimitedReader reader(String csv, Column... plan) {
        return DelimitedReader.of(csv.getBytes(StandardCharsets.UTF_8), HEADERLESS, List.of(plan));
    }

    @Test
    void aCheckedColumnReadsAViolation() {
        try (DelimitedReader reader = reader(
                "11\n7\nx\n", Column.i32(0, Constraints.i32().maximum("10").build()))) {
            Batch batch = reader.read();
            assertTrue(batch.columns().get(0).isChecked());

            assertEquals(new Violation<>(Keyword.MAXIMUM, 11, 0, 2), batch.getChecked(0, 0, Integer.class));
            assertTrue(batch.verdict(0, 0).isViolation());
            assertFalse(batch.isOk(0, 0));

            assertEquals(new Checked<>(7, false, OptionalInt.empty()), batch.getChecked(0, 1, Integer.class));
            Fault<?> fault = assertInstanceOf(Fault.class, batch.getChecked(0, 2, Integer.class));
            assertEquals(CastFailure.MALFORMED, fault.reason());
            assertEquals(batch.getChecked(0, 1, Integer.class), batch.row(1).getChecked(0, Integer.class));
        }
    }

    @Test
    void aRoundedValueIsFlaggedAdjusted() {
        Constraints<BigDecimal> declared =
                Constraints.decimal().multipleOf("0.5").rounding(Rounding.FLOOR).build();
        try (DelimitedReader reader = reader("1.3\n1.5\n", Column.decimal(0, declared))) {
            Batch batch = reader.read();
            assertEquals(
                    new Checked<>(new BigDecimal("1"), true, OptionalInt.empty()),
                    batch.getChecked(0, 0, BigDecimal.class));
            assertEquals(
                    new Checked<>(new BigDecimal("1.5"), false, OptionalInt.empty()),
                    batch.getChecked(0, 1, BigDecimal.class));
        }
    }

    @Test
    void aTextEnumMatchDeliversTheDeclaredEntry() {
        Constraints<String> declared =
                Constraints.text().enumOf("Female", "Male").build();
        try (DelimitedReader reader = reader("MALE\nother\n", Column.text(0, declared))) {
            Batch batch = reader.read();
            Checked<?> passed = assertInstanceOf(Checked.class, batch.getChecked(0, 0, String.class));
            assertEquals(new Checked<>("Male", false, OptionalInt.of(1)), passed);
            assertSame(declared.enumValues().orElseThrow().get(1), passed.value(), "the caller's own string");
            assertEquals(new Violation<>(Keyword.ENUM, "other", 0, 5), batch.getChecked(0, 1, String.class));
        }
    }

    /**
     * An unconstrained column under the checked getter: what cast passes, unadjusted, and a fault is the
     * same fault. No stale metadata can reach it: an unconstrained column is given no checks array.
     */
    @Test
    void anUnconstrainedColumnKeepsItsFaultsUnderTheCheckedGetter() {
        try (DelimitedReader reader = reader("5\n\"\"\nx\n", Column.i32(0))) {
            Batch batch = reader.read();
            assertFalse(batch.columns().get(0).isChecked());
            assertEquals(new Checked<>(5, false, OptionalInt.empty()), batch.getChecked(0, 0, Integer.class));
            assertEquals(batch.verdict(0, 1).toFault(), batch.getChecked(0, 1, Integer.class));
            assertEquals(
                    CastFailure.EMPTY,
                    assertInstanceOf(Fault.class, batch.getChecked(0, 1, Integer.class))
                            .reason());
            assertEquals(
                    CastFailure.MALFORMED,
                    assertInstanceOf(Fault.class, batch.getChecked(0, 2, Integer.class))
                            .reason());
        }
    }

    @Test
    void thePlainGettersRefuseAConstrainedColumn() {
        try (DelimitedReader reader = reader(
                "7,a\n",
                Column.i32(0, Constraints.i32().maximum("10").build()),
                Column.text(1, Constraints.text().maxLength(3).build()))) {
            Batch batch = reader.read();
            assertTrue(assertThrows(IllegalStateException.class, () -> batch.get(0, 0, Integer.class))
                    .getMessage()
                    .contains("getChecked"));
            assertThrows(IllegalStateException.class, () -> batch.string(1, 0));
            assertThrows(IllegalStateException.class, () -> batch.text(1, 0));
            assertThrows(IllegalStateException.class, () -> batch.chars(1, 0));
            assertThrows(IllegalStateException.class, () -> batch.getChars(1, 0, new char[8], 0));
            assertThrows(IllegalStateException.class, () -> batch.row(0).get(0, Integer.class));
            // The checked getter reads the door's own type, and no other.
            assertThrows(IllegalStateException.class, () -> batch.getChecked(0, 0, Long.class));
            assertThrows(IllegalStateException.class, () -> batch.getChecked(1, 0, Integer.class));
        }
    }

    @Test
    void aMismatchedDoorIsRefused() {
        // Constraints<Integer> serves i32, u8 and u16: the declaration names its door, and the column checks it.
        IllegalArgumentException refused = assertThrows(
                IllegalArgumentException.class,
                () -> Column.i32(0, Constraints.u8().build()));
        assertEquals("Constraints built for the u8 door were passed to a i32 column", refused.getMessage());
        assertThrows(NullPointerException.class, () -> Column.i32(0, (Constraints<Integer>) null));
        assertThrows(NullPointerException.class, () -> Column.text(0, (Constraints<String>) null));
    }

    @Test
    void faultsListAViolationWithItsKeyword() {
        try (DelimitedReader reader =
                reader("11\nx\n", Column.i32(0, Constraints.i32().maximum("10").build()))) {
            Batch batch = reader.read();
            List<Problem> listed = new ArrayList<>();
            batch.faults().forEach(listed::add);
            assertEquals(2, listed.size());
            assertEquals(Keyword.MAXIMUM, listed.get(0).keyword());
            assertNull(listed.get(0).reason());
            assertEquals(0, listed.get(0).offset());
            assertEquals(2, listed.get(0).length());
            assertNull(listed.get(1).keyword());
            assertEquals(CastFailure.MALFORMED, listed.get(1).reason());
        }
    }

    @Test
    void checksAreSizedLikeVerdicts() {
        try (Arena arena = Arena.ofConfined()) {
            Columns columns = new Columns(
                    new Column[] {Column.i32(0, Constraints.i32().maximum("10").build()), Column.i32(0)},
                    37,
                    arena,
                    false,
                    false,
                    false);
            assertNotNull(columns.checks[0]);
            assertEquals(
                    columns.verdicts[0].byteSize() / Abi.VERDICT_BYTES, columns.checks[0].byteSize() / Abi.CHECK_BYTES);
            assertNull(columns.checks[1], "an unconstrained column is given none");
        }
    }
}
