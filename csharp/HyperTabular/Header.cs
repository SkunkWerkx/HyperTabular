using System.Collections;
using System.Text;

namespace HyperTabular;

/// <summary>
/// A header record's names, in source order — the list of strings it always was, and the way
/// from a column's name to the ordinal a <see cref="Column"/> is built with.
/// </summary>
/// <remarks>
/// <para>
/// A name is matched exactly: ordinal, case-sensitive and untrimmed, as the Rust core's
/// <c>Header::ordinal</c> matches it. When two columns share a name, the first is the one
/// found.
/// </para>
/// <para>
/// The names are decoded from UTF-8 once, when the header is read (bytes that are not UTF-8
/// are replaced), and the bytes they were decoded from are kept, so that
/// <see cref="Ordinal(ReadOnlySpan{byte})"/> matches what the source held rather than what it
/// decoded to.
/// </para>
/// </remarks>
public sealed class Header : IReadOnlyList<string>
{
	readonly string[] _names;
	/// <summary>Every name's bytes, end to end; name <c>i</c> ends at <c>_ends[i]</c>.</summary>
	readonly byte[] _utf8;
	readonly int[] _ends;

	Header(string[] names, byte[] utf8, int[] ends)
	{
		_names = names;
		_utf8 = utf8;
		_ends = ends;
	}

	/// <summary>A header with no names: the header of an input that had no record.</summary>
	internal static readonly Header Empty = new([], [], []);

	/// <summary>Collects a header's names as the core hands them out, then makes the header.</summary>
	internal sealed class Builder(int count)
	{
		readonly string[] _names = new string[count];
		readonly int[] _ends = new int[count];
		byte[] _utf8 = new byte[64];
		int _count;
		int _length;

		/// <summary>Adds the next name's bytes.</summary>
		public void Add(ReadOnlySpan<byte> name)
		{
			if (_utf8.Length - _length < name.Length)
				Array.Resize(ref _utf8, Math.Max(_utf8.Length * 2, _length + name.Length));
			name.CopyTo(_utf8.AsSpan(_length));
			_names[_count] = Encoding.UTF8.GetString(name);
			_length += name.Length;
			_ends[_count++] = _length;
		}

		/// <summary>The header of every name added.</summary>
		public Header Build() => new(_names, _utf8.AsSpan(0, _length).ToArray(), _ends);
	}

	/// <summary>How many names the header has.</summary>
	public int Count => _names.Length;

	/// <summary>The name of the column at <paramref name="ordinal"/>.</summary>
	/// <exception cref="IndexOutOfRangeException">The header has no column at that ordinal.</exception>
	public string this[int ordinal] => _names[ordinal];

	/// <summary>The bytes the name at <paramref name="ordinal"/> was decoded from.</summary>
	/// <exception cref="ArgumentOutOfRangeException">The header has no column at that ordinal.</exception>
	public ReadOnlySpan<byte> Utf8(int ordinal)
	{
		ArgumentOutOfRangeException.ThrowIfGreaterThanOrEqual((uint)ordinal, (uint)_names.Length, nameof(ordinal));
		var start = ordinal == 0 ? 0 : _ends[ordinal - 1];
		return _utf8.AsSpan(start, _ends[ordinal] - start);
	}

	/// <summary>The ordinal of the first column named exactly <paramref name="name"/>.</summary>
	/// <exception cref="KeyNotFoundException">No column has that name; the message names it.</exception>
	public int Ordinal(string name) =>
		TryOrdinal(name, out var ordinal)
			? ordinal
			: throw new KeyNotFoundException($"The header has no column named \"{name}\".");

	/// <summary>The ordinal of the first column named exactly <paramref name="name"/>, if there is one.</summary>
	/// <returns><see langword="false"/> if no column has that name.</returns>
	public bool TryOrdinal(string name, out int ordinal)
	{
		ArgumentNullException.ThrowIfNull(name);
		ordinal = Array.IndexOf(_names, name);
		return ordinal >= 0;
	}

	/// <summary>
	/// The ordinal of the first column whose name is exactly the bytes
	/// <paramref name="utf8Name"/> — the Rust core's <c>Header::ordinal</c>, byte for byte.
	/// </summary>
	/// <exception cref="KeyNotFoundException">No column has that name; the message names it.</exception>
	public int Ordinal(ReadOnlySpan<byte> utf8Name) =>
		TryOrdinal(utf8Name, out var ordinal)
			? ordinal
			: throw new KeyNotFoundException($"The header has no column named \"{Encoding.UTF8.GetString(utf8Name)}\".");

	/// <summary>The ordinal of the first column whose name is exactly the bytes <paramref name="utf8Name"/>, if there is one.</summary>
	/// <returns><see langword="false"/> if no column has that name.</returns>
	public bool TryOrdinal(ReadOnlySpan<byte> utf8Name, out int ordinal)
	{
		var start = 0;
		for (ordinal = 0; ordinal < _ends.Length; ordinal++)
		{
			var end = _ends[ordinal];
			if (_utf8.AsSpan(start, end - start).SequenceEqual(utf8Name))
				return true;
			start = end;
		}
		ordinal = -1;
		return false;
	}

	/// <summary>The names, in source order.</summary>
	public IEnumerator<string> GetEnumerator() => ((IEnumerable<string>)_names).GetEnumerator();

	IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();

	/// <inheritdoc/>
	public override string ToString() => string.Join(", ", _names);
}
