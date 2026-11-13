package hypertabular

import (
	"math"
	"reflect"
	"strings"
	"testing"

	hypercast "github.com/SkunkWerkx/HyperCast/go"
)

// What the corpus does not reach of a column that declares HyperCast's constraints.

var unheaded = Dialect{Separator: ',', Quoting: true, SkipBlankLines: true}

func readChecked(t *testing.T, csv string, plan ...Column) *Batch {
	t.Helper()
	reader, err := NewDelimitedReaderBytes([]byte(csv), unheaded, plan)
	if err != nil {
		t.Fatal(err)
	}
	batch, err := reader.Read()
	if err != nil || batch == nil {
		t.Fatalf("a read: %v, %v", batch, err)
	}
	return batch
}

func TestACheckedColumnReadsAViolation(t *testing.T) {
	b := readChecked(t, "11\n7\nx\n", Checked(0, hypercast.Invariant, hypercast.MustConstraints[int32](hypercast.Maximum(10))))
	if !b.Columns()[0].IsChecked() {
		t.Fatal("the column declares constraints")
	}
	passed, rejection := GetChecked[int32](b, 0, 0)
	want := &hypercast.Violation[int32]{Keyword: hypercast.KeywordMaximum, Value: 11, Offset: 0, Length: 2}
	if !reflect.DeepEqual(rejection, want) || passed != (hypercast.Checked[int32]{EnumIndex: -1}) {
		t.Errorf("a violation: %v, %v", passed, rejection)
	}
	if v := b.Verdicts(0)[0]; !v.Violation() || v.OK() || v.Reason() != 0 || v.Fault() != nil {
		t.Errorf("the verdict of a violation: %+v", v)
	}
	if b.Fault(0, 0) != nil {
		t.Error("a violation is no fault: the cell cast")
	}
	if b.I32(0)[0] != 11 {
		t.Error("the value as cast is kept")
	}
	if passed, rejection := GetChecked[int32](b, 0, 1); rejection != nil || passed != (hypercast.Checked[int32]{Value: 7, EnumIndex: -1}) {
		t.Errorf("a value: %v, %v", passed, rejection)
	}
	if _, rejection := GetChecked[int32](b, 0, 2); !reflect.DeepEqual(rejection, b.Fault(0, 2)) || b.Fault(0, 2).Reason != hypercast.Malformed {
		t.Errorf("text that isn't the type: %v", rejection)
	}
	viaRow, rowRejection := CellChecked[int32](b.Row(1), 0)
	if viaBatch, batchRejection := GetChecked[int32](b, 0, 1); viaRow != viaBatch || rowRejection != batchRejection {
		t.Errorf("CellChecked %v, GetChecked %v", viaRow, viaBatch)
	}
}

func TestARoundedValueIsFlaggedAdjusted(t *testing.T) {
	b := readChecked(t, "1.3\n1.5\n", Checked(0, hypercast.Invariant,
		hypercast.MustConstraints[hypercast.Decimal](hypercast.MultipleOf("0.5"), hypercast.WithRounding(hypercast.Floor))))
	if passed, rejection := GetChecked[hypercast.Decimal](b, 0, 0); rejection != nil || !passed.Adjusted || passed.Value.String() != "1" {
		t.Errorf("rounded: %v, %v", passed, rejection)
	}
	if passed, rejection := GetChecked[hypercast.Decimal](b, 0, 1); rejection != nil || passed.Adjusted || passed.Value.String() != "1.5" {
		t.Errorf("a multiple: %v, %v", passed, rejection)
	}
}

func TestATextEnumMatchDeliversTheDeclaredEntry(t *testing.T) {
	b := readChecked(t, "MALE\nother\n", CheckedText(0, hypercast.MustConstraints[string](hypercast.Enum("Female", "Male"))))
	if passed, rejection := GetChecked[string](b, 0, 0); rejection != nil || passed != (hypercast.Checked[string]{Value: "Male", EnumIndex: 1}) {
		t.Errorf("a match: %v, %v", passed, rejection)
	}
	_, rejection := GetChecked[string](b, 0, 1)
	if want := (&hypercast.Violation[string]{Keyword: hypercast.KeywordEnum, Value: "other", Length: 5}); !reflect.DeepEqual(rejection, want) {
		t.Errorf("no match: %v", rejection)
	}
}

// An unconstrained column under the checked getter: what cast passes, unadjusted, and a fault is the
// same fault. No stale metadata can reach it: an unconstrained column is given no checks array.
func TestAnUnconstrainedColumnKeepsItsFaultsUnderTheCheckedGetter(t *testing.T) {
	b := readChecked(t, "5\n\"\"\nx\n", I32(0, hypercast.Invariant))
	if b.Columns()[0].IsChecked() || b.columns.columns[0].checks != nil {
		t.Fatal("the column declares no constraints, and has no checks")
	}
	if passed, rejection := GetChecked[int32](b, 0, 0); rejection != nil || passed != (hypercast.Checked[int32]{Value: 5, EnumIndex: -1}) {
		t.Errorf("a value: %v, %v", passed, rejection)
	}
	for row, reason := range []hypercast.CastFailure{1: hypercast.Empty, 2: hypercast.Malformed} {
		if row == 0 {
			continue
		}
		if _, rejection := GetChecked[int32](b, 0, row); !reflect.DeepEqual(rejection, b.Fault(0, row)) || b.Fault(0, row).Reason != reason {
			t.Errorf("row %d: %v, want %v", row, rejection, reason)
		}
	}
}

func TestThePlainGettersRefuseAConstrainedColumn(t *testing.T) {
	b := readChecked(t, "7,a\n",
		Checked(0, hypercast.Invariant, hypercast.MustConstraints[int32](hypercast.Maximum(10))),
		CheckedText(1, hypercast.MustConstraints[string](hypercast.MaxLength(3))))
	for name, read := range map[string]func(){
		"Get":         func() { Get[int32](b, 0, 0) },
		"Cell":        func() { Cell[int32](b.Row(0), 0) },
		"Text":        func() { b.Text(1) },
		"TextString":  func() { b.TextString(1, 0) },
		"Row.Text":    func() { b.Row(0).Text(1) },
		"Get[string]": func() { Get[string](b, 1, 0) },
		// The checked getter reads the door's own type, and no other.
		"GetChecked[int64]": func() { GetChecked[int64](b, 0, 0) },
		"GetChecked[int32]": func() { GetChecked[int32](b, 1, 0) },
	} {
		if !panics(read) {
			t.Errorf("%s read a constrained column", name)
		}
	}
	// The control: the checked getters read them.
	if _, rejection := GetChecked[int32](b, 0, 0); rejection != nil {
		t.Error(rejection)
	}
	if passed, rejection := GetChecked[string](b, 1, 0); rejection != nil || passed.Value != "a" {
		t.Error(passed, rejection)
	}
}

func TestFaultsListAViolationWithItsKeyword(t *testing.T) {
	b := readChecked(t, "11\nx\n", Checked(0, hypercast.Invariant, hypercast.MustConstraints[int32](hypercast.Maximum(10))))
	var listed []Problem
	for problem := range b.Faults() {
		listed = append(listed, problem)
	}
	if len(listed) != 2 {
		t.Fatalf("%d problems: %+v", len(listed), listed)
	}
	if p := listed[0]; p.Keyword != hypercast.KeywordMaximum || p.Reason != 0 || p.Offset != 0 || p.Length != 2 {
		t.Errorf("a violation: %+v", p)
	}
	if p := listed[1]; p.Keyword != 0 || p.Reason != hypercast.Malformed {
		t.Errorf("a fault: %+v", p)
	}
}

func TestChecksAreSizedLikeVerdicts(t *testing.T) {
	set, err := newColumns([]Column{
		Checked(0, hypercast.Invariant, hypercast.MustConstraints[int32](hypercast.Maximum(10))),
		I32(0, hypercast.Invariant),
	}, 37, true, 0)
	if err != nil {
		t.Fatal(err)
	}
	// The block holds 37 checks for the constrained column and none for the other.
	if checks, next := set.offsets[3], set.offsets[4]; next-checks != (37*uintptr(hypercast.RawCheckSize)+7)&^7 {
		t.Errorf("the constrained column's checks take %d bytes", next-checks)
	}
	if checks := set.offsets[7]; set.blockBytes != checks {
		t.Errorf("the unconstrained column's checks take %d bytes", set.blockBytes-checks)
	}
	// And the core writes the 37th beside the 37th verdict.
	b := readChecked(t, strings.Repeat("11\n", 37), Checked(0, hypercast.Invariant, hypercast.MustConstraints[int32](hypercast.Maximum(10))))
	if b.Rows() != 37 || len(b.columns.columns[0].checks) != len(b.columns.columns[0].verdicts) {
		t.Fatalf("%d rows, %d checks", b.Rows(), len(b.columns.columns[0].checks))
	}
	if _, rejection := GetChecked[int32](b, 0, 36); rejection == nil {
		t.Error("the last row's violation")
	}
}

func TestAColumnIsAValue(t *testing.T) {
	declared := func(maximum int) Column {
		return Checked(0, hypercast.Invariant, hypercast.MustConstraints[int32](hypercast.Maximum(maximum)))
	}
	if declared(10) != declared(10) || declared(10) == declared(11) || declared(10) == I32(0, hypercast.Invariant) {
		t.Error("columns compare by what they declare")
	}
	if Checked(0, hypercast.Invariant, hypercast.MustConstraints[uint8]()).Door() != DoorU8 ||
		Checked(0, hypercast.Invariant, hypercast.MustConstraints[hypercast.Decimal]()).Door() != DoorExact {
		t.Error("the constraints' type names the door")
	}
	if !panics(func() { Checked(0, hypercast.Invariant, hypercast.Constraints[int32]{}) }) {
		t.Error("the zero Constraints, never judged, is a caller bug")
	}
}

func TestATextEnumEntryIsReadOutOfThePackedList(t *testing.T) {
	column := CheckedText(0, hypercast.MustConstraints[string](hypercast.Enum("", "zoë", "x")))
	for index, entry := range []string{"", "zoë", "x"} {
		if got := column.enumEntry(uint32(index)); got != entry {
			t.Errorf("entry %d: %q, want %q", index, got, entry)
		}
	}
	if !panics(func() { column.enumEntry(3) }) {
		t.Error("an entry past the list")
	}
}

// A column that declares constraints is given a check a row, which the core counts against the
// batch's limit when the plan is bound: at the most rows the same column without a declaration
// holds, the declared one is refused, before anything for the batch is allocated.
func TestADeclaredColumnsChecksCountAgainstTheBatchLimit(t *testing.T) {
	plain := []Column{Text(0)}
	declared := []Column{CheckedText(0, hypercast.MustConstraints[string](hypercast.MaxLength(3)))}
	specs, extras := sizeSpecs(plain)
	// The most rows the plain column is allowed: the core's limit, found by halving.
	allowed := func(rows int) bool {
		code, _, err := askSizes(specs, extras, rows, 1, batchNotes)
		if err != nil {
			t.Fatal(err)
		}
		return code == codeOK
	}
	most, past := 1, math.MaxInt32
	if !allowed(most) || allowed(past) {
		t.Fatal("the plain column's limit is not between 1 and MaxInt32 rows")
	}
	for past-most > 1 {
		if mid := most + (past-most)/2; allowed(mid) {
			most = mid
		} else {
			past = mid
		}
	}
	if _, err := checkSizes(specs, extras, most, 1, batchNotes); err != nil {
		t.Fatalf("the plain column at %d rows: %v", most, err)
	}
	specs, extras = sizeSpecs(declared)
	if _, err := checkSizes(specs, extras, most, 1, batchNotes); err == nil || !strings.Contains(err.Error(), "past the limit") {
		t.Fatalf("the declared column at %d rows: %v, want past the limit", most, err)
	}
	if _, err := NewDelimitedReaderBytes([]byte("a\n"), unheaded, declared, BatchRows(most)); err == nil || !strings.Contains(err.Error(), "past the limit") {
		t.Fatalf("binding the declared column at %d rows: %v, want past the limit", most, err)
	}
}
