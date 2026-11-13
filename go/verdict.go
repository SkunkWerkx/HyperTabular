package hypertabular

import hypercast "github.com/SkunkWerkx/HyperCast/go"

// CellVerdict is one cell's verdict as the core writes it into a column's verdict array:
// HyperCast's reason code, with zero for a cell that cast and its own for a constraint broken,
// and the offending span within the cell's own text. DelimitedReader.Verdicts hands a column's verdicts out as a slice of
// these, for code that wants to scan a batch without a *hypercast.Fault per cell.
type CellVerdict struct {
	offset uint32
	length uint32
	reason uint32
}

// OK reports whether the cell cast and, in a column that declares constraints, kept them.
func (v CellVerdict) OK() bool { return v.reason == 0 }

// verdictViolation is the core's code for a cell that cast and broke a constraint.
const verdictViolation = 4

// Violation reports whether the cell cast but broke a constraint its column declares: no fault,
// and GetChecked names the keyword.
func (v CellVerdict) Violation() bool { return v.reason == verdictViolation }

// Reason is why the cell did not cast — zero, which is no CastFailure, when it did, a
// violation's included.
func (v CellVerdict) Reason() hypercast.CastFailure {
	if v.reason == verdictViolation {
		return 0
	}
	return hypercast.CastFailure(v.reason)
}

// Offset is the byte offset of the offending span within the cell's text.
func (v CellVerdict) Offset() int { return int(v.offset) }

// Length is the byte length of the offending span.
func (v CellVerdict) Length() int { return int(v.length) }

// Fault is the verdict as HyperCast's own union: nil for a cell that cast, a violation's
// included, otherwise the *hypercast.Fault the HyperCast door of the same name returns for the
// same text. The fault is the one allocation this makes.
func (v CellVerdict) Fault() *hypercast.Fault {
	if v.reason == 0 || v.reason == verdictViolation {
		return nil
	}
	fault, ok := hypercast.FaultFromCode(v.reason, v.offset, v.length)
	if !ok {
		panic(contractViolation)
	}
	return fault
}
