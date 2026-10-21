# Unused parameter (UnusedParameters)

## Description

Methods should not declare parameters that are never used in the method body.

Unused parameters make the API harder to understand and complicate call sites. The current implementation skips several categories of methods that are expected to have fixed signatures, such as platform handlers, HTTP handlers, web service operation handlers, integration service message receive handlers, attachable methods, and local callbacks registered through `NotifyDescription`.

In form modules a method bound as a handler at runtime keeps its platform-fixed signature and is skipped as well: both direct `УстановитьДействие` registrations and any method named by an identifier-shaped string literal in the same module (a command created in code, a helper module fed a parameter structure). String data that coincides with a method name therefore also exempts that method's parameters.

An exported form-module method is skipped too: it is an interface for code outside the form (for example an external data processor calling `Форма.Метод(...)` by name), so the caller dictates its parameters. Non-exported form-module methods are checked as usual.

## Examples

Incorrect:

```bsl
Function AddTwoNumbers(Val FirstValue, Val SecondValue, Val UnusedParameter)
    Return FirstValue + SecondValue;
EndFunction
```

Correct:

```bsl
Function AddTwoNumbers(Val FirstValue, Val SecondValue)
    Return FirstValue + SecondValue;
EndFunction
```

## Sources

* [v8std: UnusedParameters](https://v8std.ru/diagnostics/bslls/UnusedParameters/)
