using System.Runtime.InteropServices;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>
/// One cell's verdict as the core writes it into a column's verdict array: HyperCast's
/// reason code, with <c>0</c> for a cell that cast, and the offending span within the
/// cell's own text. <see cref="Batch.Verdicts"/> hands a column's verdicts out as
/// a span of these, for code that wants to scan a batch without a union per cell.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
public readonly struct CellVerdict
{
	readonly uint _offset;
	readonly uint _length;
	readonly uint _reason;

	/// <summary><see langword="true"/> when the cell cast.</summary>
	public bool IsOk => _reason == 0;

	/// <summary>
	/// Why the cell did not cast — <see cref="CastFailure.Unspecified"/> when it did.
	/// </summary>
	/// <exception cref="InvalidOperationException">The code names no reason — a binding bug, not data.</exception>
	public CastFailure Reason => IsOk ? CastFailure.Unspecified : ToFault().Reason;

	/// <summary>Byte offset of the offending span within the cell's text.</summary>
	public int Offset => (int)_offset;

	/// <summary>Byte length of the offending span.</summary>
	public int Length => (int)_length;

	/// <summary>The verdict as HyperCast's <see cref="Fault"/>. Only for a cell that did not cast.</summary>
	/// <exception cref="InvalidOperationException">The cell cast; there is no fault.</exception>
	public Fault ToFault() =>
		IsOk
			? throw new InvalidOperationException("The cell cast; it has no fault.")
			: Abi.ToFault(_reason, new RawFault(_offset, _length));
}
