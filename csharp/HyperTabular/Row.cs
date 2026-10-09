using HyperCast;

namespace HyperTabular;

/// <summary>
/// One row of a <see cref="Batch"/>: the batch's accessors with the row already chosen, for
/// code that thinks in records — "for each row, build an object from these columns".
/// </summary>
/// <remarks>
/// A row is a view, not a copy: a batch and an index, nothing more, made without allocating.
/// It reads the batch's column-major storage exactly as the batch's own accessors do, and is
/// valid exactly as long — until the reader is asked for the next batch. Being a
/// <see langword="ref"/> struct, it cannot be kept past that by accident.
/// </remarks>
public readonly ref struct Row
{
	readonly Batch _batch;

	internal Row(Batch batch, int index)
	{
		_batch = batch;
		Index = index;
	}

	/// <summary>The row's place in its batch, from 0.</summary>
	public int Index { get; }

	/// <summary>Where the row came from: <see cref="Batch.Line"/> of this row.</summary>
	public int Line => _batch.Line(Index);

	/// <summary>The cell in <paramref name="column"/>, as HyperCast judged it: <see cref="Batch.Get{T}"/> of this row.</summary>
	/// <exception cref="InvalidOperationException">The column's door does not make a <typeparamref name="T"/>.</exception>
	public Verdict<T> Get<T>(int column) where T : struct => _batch.Get<T>(column, Index);

	/// <summary>The verdict of the cell in <paramref name="column"/>: <see cref="Batch.Verdicts"/> of the column, at this row.</summary>
	public CellVerdict Verdict(int column) => _batch.Verdicts(column)[Index];

	/// <summary>The bytes of the text cell in <paramref name="column"/>: <see cref="Batch.TryGetText"/> of this row.</summary>
	/// <exception cref="InvalidOperationException">The column is not read through <see cref="Door.Text"/>.</exception>
	public bool TryGetText(int column, out ReadOnlySpan<byte> utf8) => _batch.TryGetText(column, Index, out utf8);

	/// <summary>The text cell in <paramref name="column"/> as a string: <see cref="Batch.GetString"/> of this row.</summary>
	/// <exception cref="InvalidOperationException">The column is not read through <see cref="Door.Text"/>.</exception>
	public string? GetString(int column) => _batch.GetString(column, Index);

	/// <summary>The text cell in <paramref name="column"/> as UTF-16, without a string: <see cref="Batch.GetChars"/> of this row.</summary>
	/// <exception cref="InvalidOperationException">The column is not read through <see cref="Door.Text"/>.</exception>
	public ReadOnlySpan<char> GetChars(int column) => _batch.GetChars(column, Index);

	/// <summary>The text cell in <paramref name="column"/> decoded into the caller's buffer: <see cref="Batch.TryGetChars"/> of this row.</summary>
	/// <exception cref="InvalidOperationException">The column is not read through <see cref="Door.Text"/>.</exception>
	public bool TryGetChars(int column, Span<char> destination, out int charsWritten) =>
		_batch.TryGetChars(column, Index, destination, out charsWritten);

	/// <summary>The text the cell in <paramref name="column"/> was cast from: <see cref="Batch.Raw"/> of this row.</summary>
	public ReadOnlySpan<byte> Raw(int column) => _batch.Raw(column, Index);
}

/// <summary>What a reader is, to a loop over its rows: something that reads the next batch.</summary>
internal interface IBatchSource
{
	Batch? Read();
}

/// <summary>
/// Every row a reader has left, across its batches: what <see cref="DelimitedReader.Rows"/>
/// and <see cref="Sheet.Rows"/> return, for <c>foreach</c>.
/// </summary>
/// <remarks>
/// The loop reads the next batch whenever the current one runs out, and the reader hands out
/// the same <see cref="Batch"/> every time, so a <see cref="Row"/> is valid for its own
/// iteration only. A failure the reader raises is raised from the loop.
/// </remarks>
public readonly struct RowSequence
{
	readonly IBatchSource _source;

	internal RowSequence(IBatchSource source) => _source = source;

	/// <summary>Starts the loop.</summary>
	public Enumerator GetEnumerator() => new(_source);

	/// <summary>The loop over a reader's rows, reading a batch whenever one runs out.</summary>
	public ref struct Enumerator
	{
		readonly IBatchSource _source;
		Batch? _batch;
		int _index;

		internal Enumerator(IBatchSource source)
		{
			_source = source;
			_index = -1;
		}

		/// <summary>The row the loop is on.</summary>
		public readonly Row Current => new(_batch!, _index);

		/// <summary>Moves to the next row, reading the next batch when this one has no more.</summary>
		public bool MoveNext()
		{
			if (_batch is not null && ++_index < _batch.Rows)
				return true;
			_batch = _source.Read();
			_index = 0;
			return _batch is not null;
		}
	}
}
