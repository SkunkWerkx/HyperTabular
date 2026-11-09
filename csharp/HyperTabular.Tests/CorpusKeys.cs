using System.Text.Json;
using System.Text.Json.Nodes;

namespace HyperTabular.Tests;

/// <summary>
/// The keys a replay reads, at every level of both corpus files: a replay fails on any other, so that a
/// key added to a corpus cannot be passed over. A workbook case's <c>note</c> is the one key no replay
/// reads, a sentence for whoever reads the file.
/// </summary>
static class CorpusKeys
{
	static readonly HashSet<string> _format = ["decimal_sep", "group_sep", "flags", "currency"];
	static readonly HashSet<string> _plan = ["door", "ordinal", "precision", "order", "epoch", "format", "tokens", "options"];
	static readonly HashSet<string> _dateOptions = ["flags", "pivot", "months"];
	static readonly HashSet<string> _cell =
	[
		"expect", "fault", "raw", "raw_hex", "detail", "origin", "value", "text", "magnitude", "negative", "scale",
		"seconds", "nanos", "year", "month", "day", "nanos_of_day",
	];
	static readonly HashSet<string> _problem =
		["row", "line", "plan_column", "source_column", "reason", "detail", "origin", "fault", "expected", "found"];
	static readonly HashSet<string> _failure = ["kind", "record", "line", "byte", "expected", "found"];
	static readonly HashSet<string> _suggest = ["columns", "truncated", "sample_rows", "format"];
	static readonly HashSet<string> _suggested = ["door", "param", "status", "matched", "sampled"];

	static readonly HashSet<string> _delimited =
		["name", "input", "input_hex", "dialect", "plan", "header", "rows", "problems", "failure", "suggest"];
	static readonly HashSet<string> _dialect =
	[
		"separator", "quoting", "has_header", "skip_blank_lines", "skip", "header_row", "short_rows", "long_rows",
		"tokens", "encoding",
	];

	static readonly HashSet<string> _workbook =
	[
		"name", "note", "file", "format", "epoch", "sheets", "sheet", "options", "plan", "header", "rows", "numbers",
		"problems", "failure", "suggest",
	];
	static readonly HashSet<string> _sheet = ["name", "hidden"];
	static readonly HashSet<string> _sheetOptions = ["has_header", "skip_empty_rows", "skip", "header_row", "range", "tokens"];

	internal static List<string> UnknownDelimited(JsonElement[] cases)
	{
		var unknown = new List<string>();
		foreach (var @case in cases)
		{
			var name = NameOf(@case);
			Check(@case, name, _delimited, unknown);
			Check(@case.GetProperty("dialect"), $"{name}.dialect", _dialect, unknown);
			CheckShared(@case, name, unknown);
		}
		return unknown;
	}

	internal static List<string> UnknownWorkbook(JsonElement[] cases)
	{
		var unknown = new List<string>();
		foreach (var @case in cases)
		{
			var name = NameOf(@case);
			Check(@case, name, _workbook, unknown);
			if (@case.TryGetProperty("options", out var options))
				Check(options, $"{name}.options", _sheetOptions, unknown);
			if (@case.TryGetProperty("sheets", out var sheets))
				foreach (var sheet in sheets.EnumerateArray())
					Check(sheet, $"{name}.sheets[]", _sheet, unknown);
			CheckShared(@case, name, unknown);
		}
		return unknown;
	}

	static string NameOf(JsonElement @case) =>
		@case.TryGetProperty("name", out var name) ? name.GetString()! : "a case with no name";

	static void CheckShared(JsonElement @case, string name, List<string> unknown)
	{
		if (@case.TryGetProperty("plan", out var plan))
			foreach (var entry in plan.EnumerateArray())
			{
				Check(entry, $"{name}.plan[]", _plan, unknown);
				if (entry.TryGetProperty("format", out var format))
					Check(format, $"{name}.plan[].format", _format, unknown);
				if (entry.TryGetProperty("options", out var options))
					Check(options, $"{name}.plan[].options", _dateOptions, unknown);
			}
		if (@case.TryGetProperty("rows", out var rows))
			foreach (var row in rows.EnumerateArray())
				foreach (var cell in row.EnumerateArray())
					Check(cell, $"{name}.rows[][]", _cell, unknown);
		if (@case.TryGetProperty("problems", out var problems))
			foreach (var problem in problems.EnumerateArray())
				Check(problem, $"{name}.problems[]", _problem, unknown);
		if (@case.TryGetProperty("failure", out var failure) && failure.ValueKind == JsonValueKind.Object)
			Check(failure, $"{name}.failure", _failure, unknown);
		if (@case.TryGetProperty("suggest", out var suggest))
		{
			Check(suggest, $"{name}.suggest", _suggest, unknown);
			if (suggest.TryGetProperty("format", out var format))
				Check(format, $"{name}.suggest.format", _format, unknown);
			foreach (var column in suggest.GetProperty("columns").EnumerateArray())
				Check(column, $"{name}.suggest.columns[]", _suggested, unknown);
		}
	}

	static void Check(JsonElement entry, string path, HashSet<string> known, List<string> unknown)
	{
		foreach (var property in entry.EnumerateObject())
			if (!known.Contains(property.Name))
				unknown.Add($"{path}.{property.Name}");
	}

	/// <summary>
	/// A made-up key put in each place the file holds an object, once per kind of place, has to be
	/// reported; the file with none of them has to report nothing.
	/// </summary>
	internal static void AJunkKeyIsSeenEverywhere(string file, Func<JsonElement[], List<string>> unknown)
	{
		var text = File.ReadAllText(Path.Combine(CorpusTests._corpusDirectory, file));
		var cases = JsonNode.Parse(text)!.AsArray();
		unknown(CorpusTests.Corpus(file)).ShouldBeEmpty();
		var places = new HashSet<string>();
		foreach (var @case in cases)
			Junk(@case!, "", node =>
			{
				node["junk_key"] = 1;
				using var document = JsonDocument.Parse(new JsonArray(@case!.DeepClone()).ToJsonString());
				var found = unknown([document.RootElement[0]]);
				node.Remove("junk_key");
				found.ShouldHaveSingleItem();
				found[0].ShouldEndWith(".junk_key");
			}, places);
		places.Count.ShouldBeGreaterThan(5);
	}

	static void Junk(JsonNode node, string place, Action<JsonObject> visit, HashSet<string> places)
	{
		switch (node)
		{
			case JsonObject entry:
				if (places.Add(place))
					visit(entry);
				foreach (var (key, value) in entry.ToArray())
					if (value is not null)
						Junk(value, $"{place}.{key}", visit, places);
				break;
			case JsonArray list:
				foreach (var value in list)
					if (value is not null)
						Junk(value, $"{place}[]", visit, places);
				break;
		}
	}
}
