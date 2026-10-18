# Commented out code (CommentedCode)

<!-- Блоки выше заполняются автоматически, не трогать -->
## Description

Modules should not keep commented-out code or temporary development markers.
Such fragments usually appear during debugging, experiments, or refactoring and
then stay in the file long after they stop being useful.

Commented code makes the module harder to read because the reader has to decide
whether the fragment is still relevant, accidentally disabled, or simply
forgotten. If the code may be needed later, version control should preserve it
instead of leaving it inline.

The diagnostic also treats developer-specific service marks as a smell when they
describe unfinished local work rather than business meaning.

## Parameters

- `skipSupported` (boolean, `false` by default) — do not report in the modules of
  objects on vendor support. In a vendor module a commented-out fragment is the
  usual way to switch vendor code off so that the change survives an update, and
  such a finding cannot be acted on. The developer's own modules are checked as
  before.

  Vendor ownership is read from `Ext/ParentConfigurations.bin` of the main
  configuration's dump: an object with a record there that is not taken off
  support is the vendor's; an object added on top of the vendor configuration has
  no record. A form module is judged by the form itself (a form has its own record,
  and a form added to a vendor object is the developer's), the application, session
  and external-connection modules by the configuration root. Extensions and
  external data processors are never on support and are always checked. Without
  the file the setting changes nothing; a file that cannot be read is reported as a
  warning with the position, and the setting changes nothing either.

  ```toml
  [diagnostics.parameters.CommentedCode]
  skipSupported = true
  ```

## Examples

### Wrong

```bsl
Procedure BeforeDelete(Failure)
    //If Failure Then
    //    Message("Temporary check");
    //EndIf;
EndProcedure
```

Also wrong:

```bsl
Procedure BeforeDelete(Failure)
    If True Then
        // Ivanov: revisit after merge
    EndIf;
EndProcedure
```

### Correct

Remove dead fragments and temporary notes before committing the module.

## Note

A comment block is treated as commented-out code when at least one line inside
the block is recognized as executable or declaration-like code.

## Sources

Primary source: [Standard: module texts (RU)](https://its.1c.ru/db/v8std#content:456:hdoc)

Secondary source: [v8std.ru: #std456](https://v8std.ru/std/456/)

Additional reference: [v8std.ru: CommentedCode](https://v8std.ru/diagnostics/bslls/CommentedCode/)
