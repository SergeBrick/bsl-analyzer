# Selecting from a parameter source without INTO (QueryParameterSourceWithoutInto)

## Description

A parameter in the query source list (`FROM &Data`) is an external data source: a value table, a tabular section or a query result. The platform selects the contents of an external source only into a temporary table. When the batch statement has no `INTO`, the query fails at execution with the error "The contents of a data object can only be selected into a temporary table".

Platform help, "INTO clause": to create a temporary table from an external source, the query text names, in its source list, the parameter that holds the external source; an external source can be a value table, a tabular section or a query result.

The diagnostic fires on a parameter in the `FROM` list or in a join of any query of the statement (the main query and the `UNION` members) when none of them has `INTO`. The parameter value type is not checked. A parameter in a value position (`IN (&List)`, a comparison, a virtual table argument) is not a source; nested queries are not checked.

Only a recovered text is checked: a string literal assigned, in the same method, to the `Text` property of a `Query` object and still its text at `Execute()` or `ExecuteBatch()`, with neither the object nor its text passed to a call. A text built by concatenation, changed by `StrReplace`, returned by a function, executed in another method, passed to the `New Query(...)` constructor, and a bare query text outside a module are not checked: their final text is unknown.

## Examples

Error — selecting from a parameter without `INTO`:
```bsl
Query = New Query;
Query.Text =
	"SELECT
	|	Data.Key AS Key,
	|	Data.Value AS Value
	|FROM
	|	&Data AS Data";
Query.SetParameter("Data", Data);
Result = Query.Execute();
```

Fix — put the data into a temporary table with a separate batch query and read it in the next one:
```bsl
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
	|	TT_Data AS Data";
Query.SetParameter("Data", Data);
Result = Query.Execute();
```

## Sources

* 1C:Enterprise platform help, query language: "INTO clause".
