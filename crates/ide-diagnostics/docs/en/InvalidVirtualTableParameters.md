# Invalid virtual table parameters (InvalidVirtualTableParameters)

## Description

This diagnostic reports a call of a register virtual table whose argument list does not fit the table's parameter list. The platform rejects such a query with "Invalid parameters".

Virtual-table parameters are read strictly by position, so two cases are checked:

* more arguments than the table has parameters. The usual cause is a condition split by a comma instead of `AND`: the extra comma shifts every following argument;
* a condition (a comparison, `IN`, `BETWEEN`, `LIKE`, `IS NULL`, a logical expression) passed in a parameter that takes a value (a period, a list of extra-dimension kinds, a row count). This happens when an account condition is put in the `ExtDimensions` position.

Parameter lists are known for the tables of information registers (`SliceFirst`, `SliceLast`), accumulation registers (`Balance`, `Turnovers`, `BalanceAndTurnovers`) and accounting registers (`Balance`, `Turnovers`, `DrCrTurnovers`, `BalanceAndTurnovers`, `RecordsWithExtDimensions`). The register kind is visible in the query text, so the check needs no metadata.

## Examples

A condition on extra dimensions split by a comma — five arguments, while the accounting `Balance` table has four parameters:
```sdbl
SELECT
    B.Resource1Balance AS Resource1Balance
FROM
    AccountingRegister.Register1.Balance(&Period, Account = &Account, , ExtDimension1 = &Value1, ExtDimension2 = &Value2) AS B
```

The fix is to join the conditions with `AND` inside the one `Condition` parameter:
```sdbl
SELECT
    B.Resource1Balance AS Resource1Balance
FROM
    AccountingRegister.Register1.Balance(&Period, Account = &Account, , ExtDimension1 = &Value1 AND ExtDimension2 = &Value2) AS B
```

## Sources

* [ITS: 1C:Enterprise query language, chapter 3 — register virtual tables](https://its.1c.ru/db/pubqlang)
