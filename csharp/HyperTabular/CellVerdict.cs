using System.Runtime.InteropServices;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>
/// One cell's verdict as the core writes it into a column's verdict array: HyperCast's
/// reason code, with <c>0</c> for a cell that cast and <c>4</c> for one that cast and broke a
/// constraint its column declares, and the offending span within the cell's own text. <see cref="Batch.Verdicts"/> hands a column's verdicts out as
/// a span of these, for code that wants to scan a batch without a union per cell.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public readonly struct CellVerdict
{
	readonly uint _offset;
	readonly uint _length;
	readonly uint _reason;

	// The verdict's code for a cell that cast and broke a constraint.
	const uint Violation = 4;

	/// <summary><see langword="true"/> when the cell cast, and broke no constraint.</summary>
	public bool IsOk => _reason == 0;

	/// <summary>
	/// <see langword="true"/> when the cell cast and broke a constraint its column declares
	/// (<see cref="Column.IsChecked"/>): the span is the judged text's, and
	/// <see cref="Batch.GetChecked{T}"/> names the keyword. Not a fault.
	/// </summary>
	public bool IsViolation => _reason == Violation;

	/// <summary>
	/// Why the cell did not cast — <see cref="CastFailure.Unspecified"/> when it did, a violation included.
	/// </summary>
	/// <exception cref="InvalidOperationException">The code names no reason — a binding bug, not data.</exception>
	public CastFailure Reason => IsOk || IsViolation ? CastFailure.Unspecified : ToFault().Reason;

	/// <summary>Byte offset of the offending span within the cell's text.</summary>
	public int Offset => (int)_offset;

	/// <summary>Byte length of the offending span.</summary>
	public int Length => (int)_length;

	/// <summary>
	/// The verdict as HyperCast's <see cref="Fault"/>, its reason and span. Only for a cell that did not cast.
	/// What failed isn't here: a verdict has no room for it, so <see cref="Batch.Fault"/> is the one with
	/// <see cref="Fault.Detail"/>.
	/// </summary>
	/// <exception cref="InvalidOperationException">The cell cast, and so has no fault, whether or not it broke a constraint.</exception>
	public Fault ToFault() =>
		IsOk || IsViolation
			? throw new InvalidOperationException("The cell cast; it has no fault.")
			: Abi.ToFault(_reason, new RawFault(_offset, _length));
}
