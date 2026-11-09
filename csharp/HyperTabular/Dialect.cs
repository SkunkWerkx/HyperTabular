using System.Buffers.Binary;
using System.Text;

namespace HyperTabular;

/// <summary>What a record with fewer cells than the width is. The numbers are the core's codes.</summary>
public enum ShortRows
{
	/// <summary>A <see cref="TabularFailure.ColumnCount"/> failure: the default.</summary>
	Reject = 0,
	/// <summary>A row whose missing cells are empty, at <see cref="Origin.Padded"/>.</summary>
	Pad = 1
}

/// <summary>What a record with more cells than the width is. The numbers are the core's codes.</summary>
public enum LongRows
{
	/// <summary>A <see cref="TabularFailure.ColumnCount"/> failure: the default.</summary>
	Reject = 0,
	/// <summary>Cut to the width, and <see cref="Batch.Faults"/> lists it with the width it had.</summary>
	Report = 1,
	/// <summary>Cut to the width, and nothing says so.</summary>
	Truncate = 2
}

/// <summary>
/// What a delimited file's bytes are, declared and never sniffed. Anything but
/// <see cref="Utf8"/> is decoded ahead of the read, so that everything a batch says, its spans
/// included, is about the decoded text. A unit that has no character is a
/// <see cref="TabularFailure.Encoding"/> failure, after every intact row before it, whose
/// <see cref="TabularException.Byte"/> alone counts the original bytes: nothing is ever replaced.
/// A workbook declares its own encoding, so this is delimited text's alone. The numbers are the
/// core's codes.
/// </summary>
public enum TextEncoding
{
	/// <summary>Read without a copy: the default. A cell that is not valid is that cell's malformed verdict.</summary>
	Utf8 = 0,
	/// <summary>As WHATWG has it: the five bytes the code page leaves undefined are their C1 controls. <c>EF BB BF</c> is three characters here, not a byte-order mark.</summary>
	Windows1252 = 1,
	/// <summary>ISO-8859-1: every byte the code point of its value.</summary>
	Latin1 = 2,
	/// <summary>UTF-16, little-endian, with or without a byte-order mark. A lone surrogate, or half a code unit at the end of the input, is the failure.</summary>
	Utf16Le = 3
}

/// <summary>
/// A list of what stands for no value: a cell whose exact, unquoted bytes are one of them is
/// empty, whatever its door, and <see cref="Batch.Origin"/> says <see cref="Origin.Token"/>. A
/// quoted cell never is one. <c>default(Tokens)</c> is none. Two lists are equal when they hold
/// the same tokens in the same order.
/// </summary>
public readonly struct Tokens : IEquatable<Tokens>
{
	/// <summary>Each token a little-endian 32-bit length, then its UTF-8 bytes, as the core reads them.</summary>
	readonly byte[]? _packed;

	/// <summary>The list of these tokens.</summary>
	/// <param name="tokens">The tokens, matched as their exact UTF-8 bytes.</param>
	/// <exception cref="ArgumentNullException">A token is <see langword="null"/>.</exception>
	public Tokens(params ReadOnlySpan<string> tokens)
	{
		var length = 0L;
		foreach (var token in tokens)
		{
			ArgumentNullException.ThrowIfNull(token);
			length += 4 + Encoding.UTF8.GetByteCount(token);
		}
		var packed = new byte[checked((int)length)];
		var at = 0;
		foreach (var token in tokens)
		{
			var written = Encoding.UTF8.GetBytes(token, packed.AsSpan(at + 4));
			BinaryPrimitives.WriteUInt32LittleEndian(packed.AsSpan(at), (uint)written);
			at += 4 + written;
		}
		_packed = packed;
	}

	/// <summary>The list as the core reads it.</summary>
	internal ReadOnlySpan<byte> Packed => _packed;

	/// <inheritdoc/>
	public bool Equals(Tokens other) => Packed.SequenceEqual(other.Packed);

	/// <inheritdoc/>
	public override bool Equals(object? obj) => obj is Tokens other && Equals(other);

	/// <inheritdoc/>
	public override int GetHashCode()
	{
		var hash = new HashCode();
		hash.AddBytes(Packed);
		return hash.ToHashCode();
	}

	/// <summary>Whether two lists hold the same tokens in the same order.</summary>
	public static bool operator ==(Tokens left, Tokens right) => left.Equals(right);

	/// <summary>Whether two lists differ.</summary>
	public static bool operator !=(Tokens left, Tokens right) => !left.Equals(right);
}

/// <summary>
/// How the text is delimited, declared by the caller. Nothing is sniffed: the separator is
/// stated, quoting is stated, the header is stated — the same stance HyperCast's
/// <c>NumFormat</c> takes for numeric notation.
/// </summary>
/// <param name="Separator">The single-byte separator: tab, or any printable ASCII character except <c>"</c>.</param>
/// <param name="Quoting">Whether <c>"</c> quotes cells (RFC 4180, <c>""</c> for a literal quote). Off, a quote is an ordinary byte.</param>
/// <param name="HasHeader">Whether the first record is a header, exposed through <see cref="DelimitedReader.Header"/> and never delivered as a row.</param>
/// <param name="SkipBlankLines">Whether a completely empty line is skipped rather than read as a one-cell row.</param>
public readonly record struct Dialect(char Separator, bool Quoting = true, bool HasHeader = true, bool SkipBlankLines = true)
{
	/// <summary>Comma-separated, quoted, with a header, blank lines skipped.</summary>
	public static readonly Dialect Csv = new(',');

	/// <summary>Tab-separated, otherwise as <see cref="Csv"/>.</summary>
	public static readonly Dialect Tsv = new('\t');

	/// <summary>Pipe-separated, otherwise as <see cref="Csv"/>.</summary>
	public static readonly Dialect Psv = new('|');

	/// <summary>The physical lines to pass over after a byte-order mark, before anything is read of them: a quote in one means nothing.</summary>
	public uint Skip { get; init; }

	/// <summary>The records to discard after those, ahead of the header if there is one. A blank line this dialect skips is not a record.</summary>
	public uint HeaderRow { get; init; }

	/// <summary>What a record with fewer cells than the width is.</summary>
	public ShortRows ShortRows { get; init; }

	/// <summary>What a record with more cells than the width is.</summary>
	public LongRows LongRows { get; init; }

	/// <summary>What the input's bytes are.</summary>
	public TextEncoding Encoding { get; init; }

	/// <summary>What stands for no value in every column without tokens of its own (<see cref="Column.WithTokens"/>).</summary>
	public Tokens Tokens { get; init; }

	/// <summary>
	/// Whether a batch keeps what <see cref="Batch.Origin"/>, <see cref="Batch.Fault"/>'s detail and
	/// <see cref="Batch.Faults"/> read: on unless this turns it off. Off, a fault's detail is
	/// <see cref="HyperCast.FaultDetail.None"/>, every origin is <see cref="Origin.Cell"/>, and a read
	/// saves the little it costs to ask a door why a cell failed and to keep the answer.
	/// </summary>
	public bool Notes { get; init; } = true;

	/// <summary>Refuses what no read can honour but the separator, which is the core's to judge.</summary>
	internal void CheckDeclared()
	{
		if (ShortRows is not (ShortRows.Reject or ShortRows.Pad))
			throw new ArgumentException($"ShortRows {(int)ShortRows} is not a policy.", "dialect");
		if (LongRows is not (LongRows.Reject or LongRows.Report or LongRows.Truncate))
			throw new ArgumentException($"LongRows {(int)LongRows} is not a policy.", "dialect");
		if (Encoding is not (TextEncoding.Utf8 or TextEncoding.Windows1252 or TextEncoding.Latin1 or TextEncoding.Utf16Le))
			throw new ArgumentException($"TextEncoding {(int)Encoding} is not an encoding.", "dialect");
	}
}
