package hypertabular

import hypercast "github.com/SkunkWerkx/HyperCast/go"

// CellVerdict is one cell's verdict as the core writes it into a column's verdict array:
// HyperCast's reason code, with zero for a cell that cast, and the offending span within
// the cell's own text. DelimitedReader.Verdicts hands a column's verdicts out as a slice of
// these, for code that wants to scan a batch without a *hypercast.Fault per cell.
type CellVerdict struct {
	offset uint32
	length uint32
	reason uint32
}

// OK reports whether the cell cast.
func (v CellVerdict) OK() bool { return v.reason == 0 }

// Reason is why the cell did not cast — zero, which is no CastFailure, when it did.
func (v CellVerdict) Reason() hypercast.CastFailure { return hypercast.CastFailure(v.reason) }

// Offset is the byte offset of the offending span within the cell's text.
func (v CellVerdict) Offset() int { return int(v.offset) }

// Length is the byte length of the offending span.
func (v CellVerdict) Length() int { return int(v.length) }

// Fault is the verdict as HyperCast's own union: nil for a cell that cast, otherwise the
// *hypercast.Fault the HyperCast door of the same name returns for the same text. The
// fault is the one allocation this makes.
func (v CellVerdict) Fault() *hypercast.Fault {
	if v.reason == 0 {
		return nil
	}
	fault, ok := hypercast.FaultFromCode(v.reason, v.offset, v.length)
	if !ok {
		panic(contractViolation)
	}
	return fault
}
