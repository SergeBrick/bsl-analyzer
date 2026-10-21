# TOTALS while putting a value table into a temporary table (QueryTotalsInValueTableInto)

## Description

A batch statement that puts a value table into a temporary table (`INTO` with a parameter source `FROM &Data`) cannot contain `TOTALS`.

Platform help, "INTO clause", note: the AUTOORDER, TOTALS, FOR UPDATE, HAVING and GROUP BY clauses cannot be used in a query that creates a temporary table from a value table.

The diagnostic fires once per statement, on the `TOTALS` keyword, when the statement has `INTO` and `TOTALS` and the parameter source is proven to be a value table: the same method has at least one `Query.SetParameter("<Name>", Value)` call, every such call passes a value table, and the method binds the parameters of that query in no other way (through `Query.Parameters` or `SetParameter` with a non-literal name). A tabular section, a query result, a value of unknown type and a parameter set outside the method do not trigger it.

Only a recovered text is checked: a string literal assigned, in the same method, to the `Text` property of a `Query` object and still its text at `Execute()` or `ExecuteBatch()`, with neither the object nor its text passed to a call. A bare query text outside a module is not checked.

## Examples

Error — `INTO` and `TOTALS` in one statement with a value table:
```bsl
Data = New ValueTable;
Query = New Query;
Query.Text =
	"SELECT
	|	Data.Key AS Key,
	|	Data.Value AS Value
	|INTO TT_Data
	|FROM
	|	&Data AS Data
	|TOTALS
	|	SUM(Value)
	|BY
	|	Key";
Query.SetParameter("Data", Data);
Result = Query.Execute();
```

Fix — compute the totals with a separate batch query reading the temporary table:
```bsl
Data = New ValueTable;
Query = New Query;
Query.Text =
	"SELECT
	|	Data.Key AS Key,
	|	Data.Value AS Value
	|INTO TT_Data
	|FROM
	|	&Data AS Data
	|;
	|
	|SELECT
	|	Data.Key AS Key,
	|	Data.Value AS Value
	|FROM
	|	TT_Data AS Data
	|TOTALS
	|	SUM(Value)
	|BY
	|	Key";
Query.SetParameter("Data", Data);
Result = Query.Execute();
```

## Sources

* 1C:Enterprise platform help, query language: "INTO clause".
