# Unknown field in a query table (UnknownFieldInQuery)

## Description

This diagnostic reports a query that references a field which does not exist on its metadata table.

To stay false-positive safe, it fires only for tables whose field model is provably complete: resolved reference objects, register main tables, and the virtual tables of information registers (`SliceFirst`, `SliceLast`), accumulation registers (`Balance`, `Turnovers`, `BalanceAndTurnovers`) and accounting registers (`Balance`, `Turnovers`, `DrCrTurnovers`, `BalanceAndTurnovers`, `RecordsWithExtDimensions`). It stays silent on incomplete models: temporary tables, subqueries, calculation-register tables, objects that resolve through an extension, and the tables of an accounting register whose chart of accounts is not found (without it the number of `ExtDimension<N>` fields is unknown).

For an accounting register with correspondence the field names follow the "Balance" property: a non-balance dimension or resource is named by its sides (`<Dimension>Dr`, `<Resource>Cr`) in the main table and in `RecordsWithExtDimensions`, and only non-balance resources have the correspondent-turnover fields of `Turnovers`. Fields whose presence depends on the call arguments (`Period`, the per-period breakdown such as `MonthPeriod`) are treated as present.

The check validates only the first hop: in `T.Field.SubField` only `Field` is checked against the table.

## Examples

Reference to a missing field on a register:
```sdbl
SELECT
    T.NoSuchField AS Value
FROM
    AccumulationRegister.Goods AS T
```

## Sources

* [ITS: query language fields](https://its.1c.ru/db/pubqlang)
