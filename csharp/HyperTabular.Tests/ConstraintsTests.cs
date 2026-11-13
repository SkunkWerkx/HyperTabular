using System.Text;
using HyperCast;

namespace HyperTabular.Tests;

/// <summary>A column that declares HyperCast's constraints: what the corpus does not reach of the binding's surface.</summary>
public sealed class ConstraintsTests
{
	static readonly Dialect _headerless = Dialect.Csv with { HasHeader = false };

	static Batch Read(DelimitedReader reader) => reader.Read().ShouldNotBeNull();

	static DelimitedReader Reader(string csv, params Column[] plan) =>
		new(Encoding.UTF8.GetBytes(csv).AsMemory(), _headerless, plan);

	[Fact]
	void A_checked_column_reads_a_violation()
	{
		using var reader = Reader("11\n7\nx\n", Column.Int32(0, new Constraints<int>(maximum: 10)));
		var batch = Read(reader);
		batch.Columns[0].IsChecked.ShouldBeTrue();

		var broken = batch.GetChecked<int>(0, 0).Value.ShouldBeOfType<Rejection<int>>().Value.ShouldBeOfType<Violation<int>>();
		broken.ShouldBe(new Violation<int>(Keyword.Maximum, 11, 0, 2));
		batch.Verdicts(0)[0].IsViolation.ShouldBeTrue();
		batch.Fault(0, 0).ShouldBeNull("a violation is no fault: the cell cast");
		batch.Values<int>(0)[0].ShouldBe(11, "the value as cast, kept");

		batch.GetChecked<int>(0, 1).Value.ShouldBe(new Checked<int>(7, false, null));
		var fault = batch.GetChecked<int>(0, 2).Value.ShouldBeOfType<Rejection<int>>().Value.ShouldBeOfType<Fault>();
		fault.Reason.ShouldBe(CastFailure.Malformed);
		batch.Row(1).GetChecked<int>(0).ShouldBe(batch.GetChecked<int>(0, 1));
	}

	[Fact]
	void A_rounded_value_is_flagged_adjusted()
	{
		using var reader = Reader("1.3\n1.5\n",
			Column.Decimal(0, new Constraints<decimal>(multipleOf: 0.5m, rounding: Rounding.Floor)));
		var batch = Read(reader);
		batch.GetChecked<decimal>(0, 0).Value.ShouldBe(new Checked<decimal>(1.0m, true, null));
		batch.GetChecked<decimal>(0, 1).Value.ShouldBe(new Checked<decimal>(1.5m, false, null));
	}

	[Fact]
	void A_text_enum_match_delivers_the_declared_entry()
	{
		var declared = new Constraints<string>(@enum: ["Female", "Male"]);
		using var reader = Reader("MALE\nother\n", Column.Text(0, declared));
		var batch = Read(reader);
		var passed = batch.GetChecked<string>(0, 0).Value.ShouldBeOfType<Checked<string>>();
		passed.ShouldBe(new Checked<string>("Male", false, 1));
		passed.Value.ShouldBeSameAs(declared.Enum![1].ToString(), "the caller's own string");
		var broken = batch.GetChecked<string>(0, 1).Value.ShouldBeOfType<Rejection<string>>().Value.ShouldBeOfType<Violation<string>>();
		broken.ShouldBe(new Violation<string>(Keyword.Enum, "other", 0, 5));
	}

	/// <summary>
	/// An unconstrained column under the checked getter: what cast passes, unadjusted, and a fault is the
	/// same fault. No stale metadata can reach it: an unconstrained column is given no checks array.
	/// </summary>
	[Fact]
	void An_unconstrained_column_keeps_its_faults_under_the_checked_getter()
	{
		using var reader = Reader("5\n\"\"\nx\n", Column.Int32(0));
		var batch = Read(reader);
		batch.Columns[0].IsChecked.ShouldBeFalse();
		batch.GetChecked<int>(0, 0).Value.ShouldBe(new Checked<int>(5, false, null));
		batch.GetChecked<int>(0, 1).Value.ShouldBe(new Rejection<int>(batch.Fault(0, 1)!.Value));
		batch.GetChecked<int>(0, 1).Value.ShouldBeOfType<Rejection<int>>().Value.ShouldBeOfType<Fault>().Reason.ShouldBe(CastFailure.Empty);
		batch.GetChecked<int>(0, 2).Value.ShouldBeOfType<Rejection<int>>().Value.ShouldBeOfType<Fault>().Reason.ShouldBe(CastFailure.Malformed);
	}

	[Fact]
	void The_plain_getters_refuse_a_constrained_column()
	{
		using var reader = Reader("7,a\n",
			Column.Int32(0, new Constraints<int>(maximum: 10)), Column.Text(1, new Constraints<string>(maxLength: 3)));
		var batch = Read(reader);
		Should.Throw<InvalidOperationException>(() => batch.Get<int>(0, 0)).Message.ShouldContain("GetChecked");
		Should.Throw<InvalidOperationException>(() => batch.TryGetText(1, 0, out _));
		Should.Throw<InvalidOperationException>(() => batch.GetString(1, 0));
		Should.Throw<InvalidOperationException>(() => batch.Row(0).Get<int>(0));
		// The checked getter reads the door's own type, and no other.
		Should.Throw<InvalidOperationException>(() => batch.GetChecked<long>(0, 0));
		Should.Throw<InvalidOperationException>(() => batch.GetChecked<int>(1, 0));
	}

	[Fact]
	void A_null_declaration_is_a_caller_bug()
	{
		Should.Throw<ArgumentNullException>(() => Column.Int32(0, (Constraints<int>)null!));
		Should.Throw<ArgumentNullException>(() => Column.Text(0, (Constraints<string>)null!));
		// A door that takes no constraints has none to take: HyperCast refuses the type itself.
		Should.Throw<NotSupportedException>(() => new Constraints<Guid>());
	}

	[Fact]
	void Faults_list_a_violation_with_its_keyword()
	{
		using var reader = Reader("11\nx\n", Column.Int32(0, new Constraints<int>(maximum: 10)));
		var batch = Read(reader);
		var listed = batch.Faults().ToArray();
		listed.Length.ShouldBe(2);
		listed[0].Keyword.ShouldBe(Keyword.Maximum);
		listed[0].Reason.ShouldBeNull();
		(listed[0].Offset, listed[0].Length).ShouldBe((0, 2));
		listed[1].Keyword.ShouldBeNull();
		listed[1].Reason.ShouldBe(CastFailure.Malformed);
	}

	[Fact]
	void Checks_are_sized_like_verdicts()
	{
		var columns = new Columns([Column.Int32(0, new Constraints<int>(maximum: 10)), Column.Int32(0)], batchRows: 37);
		columns.Checks[0].ShouldNotBeNull().Length.ShouldBe(columns.Verdicts[0].Length);
		columns.Checks[1].ShouldBeNull("an unconstrained column is given none");
	}
}
