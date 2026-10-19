# String names a missing metadata object (UnresolvedMetadataPath)

## Description

Some global context functions take a metadata object as a string rather than as code:

- `Тип("СправочникСсылка.Товары")` / `Type("CatalogRef.Товары")` — a type name;
- `ПредопределенноеЗначение("Перечисление.Цвета.Красный")` / `PredefinedValue(...)` —
  a path to a predefined value;
- `ОткрытьФорму("Справочник.Товары.ФормаСписка")`, `ПолучитьФорму(...)`,
  `ОткрытьФормуМодально(...)` (`OpenForm`, `GetForm`, `OpenFormModal`) — a form path.

The same thing written as code (`Справочники.Товары`, `Перечисления.Цвета.Красный`) is
checked against the metadata, the string is not. When the object is renamed, removed or
not carried over with the module into another configuration, the module compiles and
the call fails at run time. The platform's extended module check (configuration check)
reports such a string as «Возможно ошибочный параметр: "<string>"» ("Possibly invalid
parameter").

The diagnostic checks a string literal passed as the first argument of these functions
by the same rules as the code spelling:

- an object of the given kind exists in the configuration (extensions visible to the
  module included);
- an enum has such a value; a catalog or a chart (of accounts, characteristic types,
  calculation types) has such a predefined item; `ПустаяСсылка` (`EmptyRef`) always
  exists.

English spellings (`Type("CatalogRef.Товары")`, `PredefinedValue("Enum.Цвета.Красный")`,
`OpenForm("Catalog.Товары.ListForm")`) are checked the same way.

## When the diagnostic is silent

- the argument is not a string literal: a variable, a concatenation
  (`"Справочник." + Name`);
- the module belongs neither to the main configuration nor to its extension: an
  external data processor or report, a file outside the configuration roots, a project
  without the main configuration (extensions only, external objects only, or a
  directory of sources without `Configuration.xml`);
- the path does not lead into the configuration: `ВнешняяОбработка.`, `ВнешнийОтчет.`;
- a kind the analyzer's configuration model does not hold: `ОбщаяФорма.`,
  `ЖурналДокументов.`, `ОпределяемыйТип.` and the like;
- a form name: the model does not list an object's forms, and the standard ones
  (`ФормаСписка`, `ФормаВыбора`, …) exist without being declared — only the object is
  checked;
- a module method or a global common module export of the same name takes the call
  over;
- the object or the value is added by another extension of the workspace: the module
  does not see it, but every extension of the infobase is applied when the call runs;
- the value of a catalog or a chart whose predefined items were not read (no
  `Ext/Predefined.xml`, or one that does not parse, in the configuration or in an
  extension), and the value of an object adopted by an extension whose main
  configuration is not visible: the full set of values is unknown;
- the call sits in an `#If` branch no environment of the method compiles, or in a
  common module with no environments set.

## Examples

Incorrect (the configuration has no catalog `Услуги` and no value `Зеленый` in the
enum `Цвета`):

```bsl
If TypeOf(Value) = Type("CatalogRef.Услуги") Then
    Color = PredefinedValue("Enum.Цвета.Зеленый");
EndIf;
```

Correct:

```bsl
If TypeOf(Value) = Type("CatalogRef.Товары") Then
    Color = PredefinedValue("Enum.Цвета.Красный");
EndIf;
```
