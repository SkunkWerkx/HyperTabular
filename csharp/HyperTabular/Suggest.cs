using System.Numerics;
using HyperCast;
using HyperCast.Interop;

namespace HyperTabular;

/// <summary>
/// How a sample is taken and read: what <see cref="DelimitedReader.Suggest(string, Dialect, SuggestOptions?)"/>
/// and <see cref="Sheet.Suggest"/> are told. Nothing declared, it takes 1,000 rows.
/// </summary>
public sealed class SuggestOptions
{
	internal static readonly SuggestOptions Default = new();

	/// <summary>The most rows the sample holds, and always at least one: 0 is 1, and a negative number is refused. It stops at <see cref="DelimitedReader.DefaultBatchRows"/> rows of delimited text, and at a sheet's own batch size, read in as many batches as the core says fit: a sample is never refused for the size of its batch.</summary>
	public int SampleRows { get; init; } = 1000;

	/// <summary>Reads every column's numbers in this notation, which settles what <c>,</c> means in them.</summary>
	public NumFormat? Format { get; init; }

	/// <summary>Reads the numbers of a source column, by ordinal, in a notation of its own, ahead of <see cref="Format"/>.</summary>
	public IReadOnlyDictionary<int, NumFormat>? ColumnFormats { get; init; }

	/// <summary>
	/// Reads every column's dates in this order where the sample leaves the order open, as
	/// <c>01/02/2026</c> does. A column whose cells rule that order out keeps the order they leave: it
	/// answers what the sample cannot, and does not overrule what it can.
	/// </summary>
	public DateOrder? DateOrder { get; init; }

	/// <summary>Reads the dates of a source column, by ordinal, in an order of its own, ahead of <see cref="DateOrder"/>, and as that does.</summary>
	public IReadOnlyDictionary<int, DateOrder>? ColumnDateOrders { get; init; }

	internal NumFormat? DeclaredFormat(int ordinal) =>
		ColumnFormats is not null && ColumnFormats.TryGetValue(ordinal, out var format) ? format : Format;

	internal DateOrder? DeclaredOrder(int ordinal) =>
		ColumnDateOrders is not null && ColumnDateOrders.TryGetValue(ordinal, out var order) ? order : DateOrder;

	/// <summary>
	/// Judges every format these options declare as a plan's would be, whichever would win for a column
	/// and whether or not the sample reaches its ordinal. A suggest asks this before it opens or reads
	/// anything, so a stale format is refused on an empty source and on a broken one.
	/// </summary>
	internal void CheckFormats()
	{
		if (Format is { } format)
			Judge(format, "The format declared for every column");
		if (ColumnFormats is null)
			return;
		foreach (var (ordinal, declared) in ColumnFormats.OrderBy(entry => entry.Key))
			Judge(declared, $"Plan column {ordinal}'s format");

		static void Judge(NumFormat format, string what)
		{
			try
			{
				format.ToRaw();
			}
			catch (ArgumentException invalid)
			{
				throw new ArgumentException($"{what} is a NumFormat no door accepts; the inner exception says why.", "options", invalid);
			}
		}
	}

	/// <summary>How many rows to take, where a batch is <paramref name="batchRows"/>.</summary>
	internal int RowsWithin(int batchRows)
	{
		if (SampleRows < 0)
			throw new ArgumentOutOfRangeException("options", SampleRows, "SampleRows must not be negative.");
		return Math.Min(Math.Max(SampleRows, 1), Math.Max(batchRows, 1));
	}
}

/// <summary>How far a sample settles a column. The numbers are the core's codes.</summary>
public enum SuggestStatus
{
	/// <summary>Sentinel CLR default — never produced.</summary>
	Unspecified = 0,
	/// <summary>One door and one declaration take every cell, and the plan reads the column through them.</summary>
	Resolved = 1,
	/// <summary>Every cell is read, but the sample cannot say which declaration it is written in: <see cref="SuggestedColumn.Candidates"/> are the choices. The plan reads the column as text until the caller chooses.</summary>
	Ambiguous = 2,
	/// <summary>There is nothing to go on, or a cell no door took beside cells one did, or a cell that is not text at all. The plan reads the column as text.</summary>
	Unresolved = 3
}

/// <summary>A declaration that would settle a <see cref="SuggestStatus.Ambiguous"/> column: a date's order, or the notation a number is in.</summary>
/// <param name="Order">The order the date's fields come in; <see langword="null"/> for a number.</param>
/// <param name="Format">
/// The notation the numbers are in; <see langword="null"/> for a date. Declare it with
/// <see cref="SuggestOptions.ColumnFormats"/> and ask again: the door can differ under each, as
/// <c>1,234</c> is an <see cref="int"/> under one and a <see cref="double"/> under the other.
/// </param>
public readonly record struct Candidate(DateOrder? Order, NumFormat? Format);

/// <summary>What a sample says of one source column.</summary>
public sealed class SuggestedColumn
{
	internal SuggestedColumn(int ordinal, string? name, Door door, uint param, NumFormat? format, Candidate[] candidates, SuggestStatus status, int matched, int sampled)
	{
		Ordinal = ordinal;
		Name = name;
		Door = door;
		Param = param;
		Format = format;
		Candidates = candidates;
		Status = status;
		Matched = matched;
		Sampled = sampled;
	}

	/// <summary>The zero-based source column, as a <see cref="Column"/> names it.</summary>
	public int Ordinal { get; }

	/// <summary>The header's name for it, or <see langword="null"/> if the source has no header.</summary>
	public string? Name { get; }

	/// <summary>The door found: <see cref="Door.Text"/> when unresolved, and the first candidate's when ambiguous.</summary>
	public Door Door { get; }

	/// <summary>What <see cref="Door"/> declares, as the core numbers it: a date's <see cref="DateOrder"/>, the first candidate's when ambiguous, a Unix precision or an Excel epoch. 0 for another door.</summary>
	public uint Param { get; }

	/// <summary>
	/// The notation the plan reads a resolved numeric column in: the one declared for it, or else the
	/// one candidate the sample left, comma decimal where only that reads the column, and otherwise
	/// <c>,</c> grouping in threes, whether or not the sample held a <c>,</c>. Both are Western, so a later
	/// <c>1,23,456</c> is refused. <see langword="null"/> for any other column.
	/// </summary>
	public NumFormat? Format { get; }

	/// <summary>What to choose between: empty unless ambiguous.</summary>
	public IReadOnlyList<Candidate> Candidates { get; }

	/// <summary>How far the sample settles it.</summary>
	public SuggestStatus Status { get; }

	/// <summary>The cells <see cref="Door"/> took. Resolved or ambiguous, that is all of <see cref="Sampled"/>. Unresolved, it is the most that any one door but the text door took.</summary>
	public int Matched { get; }

	/// <summary>The cells that are not blank.</summary>
	public int Sampled { get; }
}

/// <summary>What a suggest found: a plan to change or to hand to a read, and why. It is never applied.</summary>
public sealed class Suggestion
{
	internal Suggestion(Column[] plan, SuggestedColumn[] report, bool truncated)
	{
		Plan = plan;
		Report = report;
		Truncated = truncated;
	}

	/// <summary>A column for each source column, in order. Only a resolved column reads through its door: the rest read as text, which refuses nothing and loses nothing.</summary>
	public IReadOnlyList<Column> Plan { get; }

	/// <summary>A report for each source column, in the plan's order.</summary>
	public IReadOnlyList<SuggestedColumn> Report { get; }

	/// <summary>Whether the source has rows past the sample. A source with exactly the sample's rows has not.</summary>
	public bool Truncated { get; }
}

/// <summary>A sample, in the shape the core takes it: each column's spans into text.</summary>
sealed unsafe class Sample
{
	/// <summary>Bit 1 of a numeric param: a format with <c>,</c> grouping digits in threes. Western.</summary>
	const uint CommaGroups = 1 << 1;
	/// <summary>Bit 2 of a numeric param: a format with <c>,</c> as the decimal separator. Western.</summary>
	const uint CommaDecimal = 1 << 2;

	readonly int _limit;
	readonly List<Native.RawSpan>[] _columns;
	/// <summary>A byte for each span of a sheet's: not 0 for a typed cell. <see langword="null"/> for delimited text.</summary>
	readonly List<byte>[]? _typed;
	byte[] _text = new byte[4096];
	int _length;
	int _rows;
	bool _truncated;

	Sample(int width, int limit, bool sheet)
	{
		_limit = limit;
		_columns = new List<Native.RawSpan>[width];
		for (var column = 0; column < width; column++)
			_columns[column] = [];
		if (sheet)
		{
			_typed = new List<byte>[width];
			for (var column = 0; column < width; column++)
				_typed[column] = [];
		}
	}

	/// <summary>
	/// Reads a source of <paramref name="width"/> columns through the text door until the sample is whole
	/// and a row past it, or the end, says whether it is the source's last. A failure among the sampled
	/// rows is the caller's, and a failure just past them only says there is more.
	/// </summary>
	/// <remarks>
	/// A sheet's sample binds a second, UUID, door to every column, for what it says of a cell: no door
	/// but a text door takes a typed cell's kind, so there it is malformed with no detail, where a text
	/// cell that is not a UUID has one. That is the read's word for which cells were typed, and it is what
	/// keeps a typed number out of the question of what <c>,</c> and <c>.</c> mean.
	/// </remarks>
	internal static Sample Of(IBatchSource source, Action<Column[]> bind, int width, int limit, bool sheet)
	{
		if (width > Columns.MaxOrdinal + 1)
			throw new ArgumentException($"A suggestion cannot plan for {width} columns; an ordinal must be below 2^20 (1,048,576).", "plan");
		var sample = new Sample(width, limit, sheet);
		if (width == 0)
			return sample;
		var plan = new Column[sheet ? 2 * width : width];
		for (var ordinal = 0; ordinal < width; ordinal++)
		{
			plan[ordinal] = Column.Text(ordinal);
			if (sheet)
				plan[width + ordinal] = Column.Uuid(ordinal);
		}
		bind(plan);
		while (!sample._truncated)
		{
			Batch? batch;
			try
			{
				batch = source.Read();
			}
			catch (Exception e) when (e is TabularException or IOException && sample._rows == limit)
			{
				sample._truncated = true;
				break;
			}
			if (batch is null)
				break;
			sample.Take(batch);
		}
		return sample;
	}

	/// <summary>
	/// Takes what the sample still wants of a batch. A blank, a token and padding are all a blank span.
	/// A refused cell keeps its bytes, for the core's own text door to refuse.
	/// </summary>
	void Take(Batch batch)
	{
		var width = _columns.Length;
		for (var row = 0; row < batch.Rows; row++)
		{
			if (_rows == _limit)
			{
				_truncated = true;
				return;
			}
			var start = _length;
			var fits = true;
			for (var column = 0; column < width && fits; column++)
			{
				var verdict = batch.Verdicts(column)[row];
				var bytes = verdict.IsOk
					? Text(batch, column, row)
					: verdict.Reason != CastFailure.Empty ? batch.Raw(column, row) : default;
				fits = Append(bytes, column);
			}
			// A span holds a 32-bit offset, and an array stops before that. A sample that far along
			// stops, and says it did.
			if (!fits)
			{
				_length = start;
				foreach (var spans in _columns)
					spans.RemoveRange(_rows, spans.Count - _rows);
				_truncated = true;
				return;
			}
			if (_typed is not null)
				for (var column = 0; column < width; column++)
				{
					var second = width + column;
					var mark = batch.Verdicts(second)[row].Reason == CastFailure.Malformed && batch.Note(second, row).Detail == 0;
					_typed[column].Add(mark ? (byte)1 : (byte)0);
				}
			_rows++;
		}
	}

	static ReadOnlySpan<byte> Text(Batch batch, int column, int row)
	{
		batch.TryGetText(column, row, out var utf8);
		return utf8;
	}

	bool Append(ReadOnlySpan<byte> bytes, int column)
	{
		var end = (long)_length + bytes.Length;
		if (end > Array.MaxLength)
			return false;
		if (end > _text.Length)
			Array.Resize(ref _text, (int)Math.Min(Math.Max(end, (long)_text.Length * 2), Array.MaxLength));
		bytes.CopyTo(_text.AsSpan(_length));
		_columns[column].Add(new Native.RawSpan { Offset = (uint)_length, Len = (uint)bytes.Length });
		_length = (int)end;
		return true;
	}

	/// <summary>Asks the core, and builds the plan and the report from its answer.</summary>
	internal Suggestion Judge(Header? header, SuggestOptions options)
	{
		var width = _columns.Length;
		var declared = new NumFormat?[width];
		var formats = new RawNumFormat[width];
		for (var ordinal = 0; ordinal < width; ordinal++)
			if (options.DeclaredFormat(ordinal) is { } format)
			{
				declared[ordinal] = format;
				formats[ordinal] = format.ToRaw(); // judged already, by CheckFormats
			}
		var cells = new Native.RawSpan[width * _rows];
		var typed = _typed is null ? null : new byte[width * _rows];
		for (var column = 0; column < width; column++)
		{
			_columns[column].CopyTo(cells, column * _rows);
			_typed?[column].CopyTo(typed!, column * _rows);
		}
		var found = new Native.RawSuggestion[width];
		fixed (byte* input = _text)
		fixed (Native.RawSpan* spans = cells)
		fixed (byte* marks = typed)
		fixed (RawNumFormat* raws = formats)
		fixed (Native.RawSuggestion* output = found)
		{
			var code = Native.hypertabular_suggest(input, (nuint)_length, null, 0, spans, marks, (nuint)_rows, (nuint)width, raws, output);
			if (code != Native.Ok)
				throw new InvalidOperationException(Workbook.ContractViolation);
		}
		var plan = new Column[width];
		var report = new SuggestedColumn[width];
		for (var ordinal = 0; ordinal < width; ordinal++)
		{
			report[ordinal] = Settled(ReportOf(ordinal, found[ordinal], header, declared[ordinal]), options.DeclaredOrder(ordinal));
			plan[ordinal] = PlanOf(report[ordinal]);
		}
		return new Suggestion(plan, report, _truncated);
	}

	/// <summary>
	/// What the core said of a column, in this binding's terms. Every door the core has is taken here, not
	/// only those it suggests today: a door it does not name, or a param its door does not declare, reads
	/// as text.
	/// </summary>
	internal static SuggestedColumn ReportOf(int ordinal, in Native.RawSuggestion found, Header? header, NumFormat? declared)
	{
		var status = (SuggestStatus)found.Status is SuggestStatus.Resolved or SuggestStatus.Ambiguous
			? (SuggestStatus)found.Status
			: SuggestStatus.Unresolved;
		var name = header is not null && ordinal < header.Count ? header[ordinal] : null;
		var door = (Door)found.Door;
		var param = found.Param;
		Candidate[] candidates = [];
		if (status == SuggestStatus.Ambiguous)
		{
			// An ambiguous param is a set of candidates, bit n for code n.
			candidates = CandidatesOf(door, found.Param);
			param = (uint)BitOperations.TrailingZeroCount(found.Param);
		}
		if (Column.IsNumeric(door))
			param = 0;
		(door, param) = DoorOf(door, param);
		NumFormat? format = null;
		if (status == SuggestStatus.Resolved && Column.IsNumeric(door))
			format = declared ?? (found.Param == CommaDecimal ? CommaDecimalFormat : NumFormat.Invariant);
		return new SuggestedColumn(ordinal, name, door, param, format, candidates, status, (int)found.Matched, (int)found.Sampled);
	}

	static readonly NumFormat CommaDecimalFormat = new(',', '.', NumStyles.All);

	/// <summary>Answers a date the sample left open with the order declared for its column, if that is one of the candidates.</summary>
	static SuggestedColumn Settled(SuggestedColumn report, DateOrder? order)
	{
		if (order is null || report.Status != SuggestStatus.Ambiguous)
			return report;
		foreach (var candidate in report.Candidates)
			if (candidate.Format is null && candidate.Order == order)
				return new SuggestedColumn(report.Ordinal, report.Name, Door.DateOrdered, Abi.Code(order.Value), null, [],
					SuggestStatus.Resolved, report.Matched, report.Sampled);
		return report;
	}

	/// <summary>A door and what it declares, or the text door when the core's numbers are not one.</summary>
	static (Door Door, uint Param) DoorOf(Door door, uint param) =>
		door switch
		{
			Door.Unix when Abi.UnixPrecisionFrom(param) is not null => (door, param),
			Door.DateOrdered or Door.DateTime when Abi.DateOrderFrom(param) is not null => (door, param),
			Door.ExcelSerial when Abi.ExcelEpochFrom(param) is not null => (door, param),
			Door.Unix or Door.DateOrdered or Door.DateTime or Door.ExcelSerial => (Door.Text, 0),
			>= Door.Boolean and <= Door.ExcelSerial => (door, 0),
			_ => (Door.Text, 0),
		};

	/// <summary>What an ambiguous column's param offers.</summary>
	static Candidate[] CandidatesOf(Door door, uint param)
	{
		var candidates = new List<Candidate>();
		if (door is Door.DateOrdered or Door.DateTime)
		{
			foreach (var order in (ReadOnlySpan<DateOrder>)[DateOrder.YearMonthDay, DateOrder.MonthDayYear, DateOrder.DayMonthYear])
				if ((param & (1u << (int)Abi.Code(order))) != 0)
					candidates.Add(new Candidate(order, null));
			return [.. candidates];
		}
		if ((param & CommaGroups) != 0)
			candidates.Add(new Candidate(null, NumFormat.Invariant));
		if ((param & CommaDecimal) != 0)
			candidates.Add(new Candidate(null, CommaDecimalFormat));
		return [.. candidates];
	}

	/// <summary>The column a report plans: only a resolved column reads through its door, a numeric one in the format the report names.</summary>
	internal static Column PlanOf(SuggestedColumn report) =>
		report.Status == SuggestStatus.Resolved
			? new Column(report.Ordinal, report.Door, report.Param, report.Format ?? NumFormat.Invariant)
			: Column.Text(report.Ordinal);
}
