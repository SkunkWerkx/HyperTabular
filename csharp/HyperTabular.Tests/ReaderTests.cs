using System.Text;
using HyperCast;

namespace HyperTabular.Tests;

/// <summary>What the corpus does not reach: the binding's own surface and its caller bugs.</summary>
public sealed class ReaderTests
{
	static readonly byte[] _orders = Encoding.UTF8.GetBytes(
		"id,name,score\n1,alice,2.5\n2,\"bob, jr\",x\n3,,7\n");

	[Fact]
	void The_native_library_answers_the_probe()
	{
		Tabular.IsAvailable.ShouldBeTrue();
		Tabular.NativeVersion.ShouldNotBeNull();
	}

	[Fact]
	void A_column_is_a_span_and_a_cell_is_a_union()
	{
		Column[] plan = [Column.Int32(0), Column.Text(1), Column.Double(2)];
		using var reader = new DelimitedReader(_orders.AsMemory(), Dialect.Csv, plan);
		reader.Header.ShouldBe(["id", "name", "score"]);
		reader.Read().ShouldBeTrue();
		reader.Rows.ShouldBe(3);

		reader.Values<int>(0).ToArray().ShouldBe([1, 2, 3]);
		reader.Values<double>(2).ToArray().ShouldBe([2.5, 0.0, 7.0]);
		reader.Verdicts(2)[1].Reason.ShouldBe(CastFailure.Malformed);

		var described = reader.Double(2, 1) switch
		{
			Success<double> score => $"{score.Value}",
			Fault fault => $"{fault.Reason} in \"{Encoding.UTF8.GetString(reader.Raw(2, 1))}\"",
		};
		described.ShouldBe("Malformed in \"x\"");
		reader.GetString(1, 1).ShouldBe("bob, jr");
		reader.GetString(1, 2).ShouldBeNull();
		reader.Read().ShouldBeFalse();
		reader.Rows.ShouldBe(0);
		reader.Records.ShouldBe(4);
	}

	[Fact]
	void Reading_a_column_as_the_wrong_type_is_a_caller_bug()
	{
		Column[] plan = [Column.Int32(0), Column.Timestamp(1)];
		using var reader = new DelimitedReader(
			Encoding.UTF8.GetBytes("1,2024-01-31T10:30:00Z\n").AsMemory(), Dialect.Csv with { HasHeader = false }, plan);
		reader.Read().ShouldBeTrue();
		Should.Throw<InvalidOperationException>(() => reader.Values<long>(0));
		Should.Throw<InvalidOperationException>(() => reader.Values<long>(1));
		Should.Throw<InvalidOperationException>(() => reader.Int64(0, 0));
		Should.Throw<ArgumentOutOfRangeException>(() => reader.Int32(0, 1));
	}

	[Fact]
	void A_plan_or_a_dialect_the_core_cannot_honour_is_refused_up_front()
	{
		var colliding = new NumFormat(',', ',', NumStyles.All);
		Should.Throw<ArgumentException>(() => new DelimitedReader(_orders.AsMemory(), Dialect.Csv, [Column.Double(0, colliding)]));
		Should.Throw<ArgumentException>(() => new DelimitedReader(_orders.AsMemory(), new Dialect('"'), [Column.Int32(0)]));
		Should.Throw<ArgumentException>(() => new DelimitedReader(_orders.AsMemory(), new Dialect('é'), [Column.Int32(0)]));
		Should.Throw<ArgumentException>(() => new DelimitedReader(_orders.AsMemory(), Dialect.Csv, [default]));
		Should.Throw<ArgumentOutOfRangeException>(() => Column.Unix(0, UnixPrecision.Unspecified));
		Should.Throw<ArgumentOutOfRangeException>(() => Column.Date(0, DateOrder.Unspecified));
		Should.Throw<ArgumentOutOfRangeException>(() => Column.ExcelSerial(0, ExcelEpoch.Unspecified));
	}

	[Fact]
	void A_file_is_read_through_the_same_reader()
	{
		var path = Path.GetTempFileName();
		try
		{
			using (var file = File.Create(path))
			{
				file.Write("n\n"u8);
				for (var row = 0; row < 100_000; row++)
					file.Write(Encoding.UTF8.GetBytes($"{row}\n"));
			}
			using var reader = DelimitedReader.Open(path, Dialect.Csv, [Column.Int64(0)], bufferBytes: 4096);
			long sum = 0, rows = 0;
			while (reader.Read())
			{
				foreach (var value in reader.Values<long>(0))
					sum += value;
				rows += reader.Rows;
			}
			(rows, sum).ShouldBe((100_000L, 4_999_950_000L));
		}
		finally
		{
			File.Delete(path);
		}
	}
}
