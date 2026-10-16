# План: обязательные проверки CI, не зависящие от нагрузки

База: `5b65205daa8ac86daf02ac7c21851fb85e573f27` (ветка `pio/ci-load-independent-tests`).
Отдельный PR; PR #444 не меняется и не закрывается.

## Задача

Требование владельца: результат обязательных проверок PR не должен зависеть
от скорости машины, планирования потоков и доступности ресурсов. Если проверку
нельзя надёжно перепроектировать, её выводят из обязательного прогона и
оставляют opt-in запуск.

Обязательный прогон PR — это job `check` (`.github/workflows/ci.yml:15-312`, его
последний шаг — `cargo test --all --no-fail-fast`, `:312`), job `corpus-contract`
(`:314-345`) и job `windows-mcp` (`:347-791`). У них есть явные фильтры
`run_exact` и `run_nonempty`. Job `partitioned-baseline-scale` идёт только вне
PR (`:798`, `if: github.event_name != 'pull_request'`).

Каждый тест из границы (раздел «Устройство сейчас», п. 3) получает одно из двух
решений:

- **(а)** функциональный контракт проверяется без wall-clock: детерминированной
  синхронизацией, управляемым временем или инвариантом;
- **(б)** тест или его временное утверждение уходит в opt-in профиль
  `timing_contract` и остаётся запускаемым.

Срез, который делает PR, задают шаги 1–8.

## Вне рамок

1. **Production-код и runtime-поведение.** Правки разрешены только в трёх
   местах:
   - тестовый код: модули, объявленные под `#[cfg(test)]` (`mod tests`,
     `graph/test_support.rs`, `diagnostics_state/test_support.rs`,
     `change_hub::test_support`, `embedder/payload_tests.rs`), файлы
     `*_tests.rs`, `tests/`;
   - `Cargo.toml` — одна строка `check-cfg`;
   - документация.

   Новых `#[cfg(test)]`-хуков в непроверочном коде нет; существующие можно
   использовать.
2. **PR #444 и его ветка.**
3. **Причина паники сервера в
   `excluding_and_readmitting_a_provider_tombstones_and_restores_its_definition`**
   (run 38800079084, см. п. 1 «Устройства»). Это настоящая паника процесса
   LSP, а не зависимость теста от нагрузки. Вынесена в «Открытые вопросы».
4. **Класс «`wait_ready`, затем чтение состояния фонового прохода»** без
   замера времени: 61 место (A4). Массово заменить на `wait_settled`
   (`crates/mcp-server/src/graph/test_support.rs:222-228`) нельзя:
   `try_publish_stale_and_catch_up` прохода не запускает (`:201-202`), и
   ожидание упрётся в `WAIT_CEILING`. В PR — только места с падениями (шаг 6).
5. **Падения, причина которых — возможный дефект продукта или не установлена.**
   Сюда входят:
   - `effective_settings_compatibility_and_execution_context_match_live_adapters`
     (`TransportClosed`, `crates/bsl-analyzer/tests/shared_workspace_analysis_s5.rs:192`,
     run 38732106909);
   - `a_revision_raised_during_an_offer_survives_that_offers_reply`
     (`crates/mcp-server/src/graph/state.rs:3862`);
   - `a_decision_that_claims_no_build_spends_no_mark_attempt` (`state.rs:9787`);
   - `workspace_cache_default_base_inside_nested_workspace_is_rejected_before_creation`
     (`crates/mcp-server/src/cache.rs:998`, Windows).

   Времени в утверждениях нет; маскировать возможный дефект нельзя. Все
   четыре — в «Открытых вопросах».
6. **Четыре теста job `partitioned-baseline-scale`** (`ci.yml:793-807`): job
   идёт только вне PR.
7. **Тесты под `#[ignore]` вне opt-in профиля**, например
   `crates/bsl-search/src/external_baseline/postgres.rs:6535`, которому нужен
   `BSL_TEST_PG_URL`. Обязательный прогон их не запускает.
8. **CI-job для `timing_contract`** — профиль запускается вручную (шаг 1).

## Допущения, принятые без вопроса

1. **Порог сторожа — 10 с.** Срок ≥ 10 с — сторож зависания, вне границы;
   срок < 10 с, который падает на корректном коде при замедлении, — в
   границе. Нормальная задержка событий здесь — миллисекунды, запас — три-четыре
   порядка. На этом пороге построены сканеры A2, A10 и A11.
2. **Срок положительного ожидания поднимается до общего потолка, а не до
   10 с.** Ожидание события, которое при регрессии не наступит никогда,
   ограничивается так:
   - в `crates/mcp-server/src/` — существующая
     `change_hub::test_support::STATE_CEILING` = 60 с (`change_hub.rs:5804`),
     в тестах графа — `graph::test_support::WAIT_CEILING` = 30 с
     (`graph/test_support.rs:76`);
   - в `crates/mcp-server/tests/` — существующая `common::STATE_CEILING` = 60 с
     (`crates/mcp-server/tests/common/mod.rs:20`) с `common::Patience` (`:23`);
   - в остальных крейтах общей константы нет. Каждый затронутый тестовый
     модуль вводит **новую** константу `HANG_WATCHDOG` = 60 с, равную
     `STATE_CEILING`.

   Другие числа не вводятся. Если тест сам задаёт срок ожидания (например
   10 с или 30 с), этот срок тоже заменяется потолком. Существующие помощники
   со сторожем ≥ 10 с, например `CLIENT_RESPONSE_HANG_GUARD` (шаг 2), не
   меняются.
3. **Граница «половина бюджета».** Если регрессия видна только по тому, что
   код прождал бюджет B (константу продукта или значение, которое тест сам
   передаёт продукту, как `WatchWaitPolicy::new`, `sync.rs:176`), а B ≥ 20 с, утверждение остаётся в
   обязательном прогоне в форме `elapsed < B / 2`. Такая граница ≥ 10 с,
   поэтому она одновременно детектор и сторож. Если B < 20 с, а наблюдаемого
   состояния без продуктового хука нет, временное утверждение уходит в (б).
   Если регрессия — бесконечное ожидание, а не ожидание бюджета, достаточно
   В.
4. **Ни одно временное утверждение не удаляется.** Исходное сравнение
   сохраняется под `if cfg!(timing_contract) { … }`, если в обычном профиле
   его заменяет одно из решений:
   - детерминированная проверка (Н, И, П);
   - общий потолок (В);
   - граница Б/2.

   Так opt-in профиль проверяет то же, что проверял раньше.
5. **Профиль называется `timing_contract`** и устроен по образцу
   `corpus_contract`:
   - `Cargo.toml:20` — `check-cfg`;
   - `docs/contributing/DEVELOPMENT_RULES.md:279-310` — описание;
   - 512 атрибутов `cfg_attr(not(corpus_contract), …)` в `crates/` — команда
     A5.

   Все крейты, кроме `check-contract`, наследуют `[workspace.lints]` (команда
   A6), а `check-contract` в PR не меняется. Поэтому одной строки `check-cfg`
   достаточно, чтобы `clippy -D warnings` не падал на `unexpected_cfgs`.
6. **Новое требование владельца сильнее прежнего решения.** Ревью в
   `b883b997` сознательно вернуло часть границ латентности как «единственный
   детектор задержки» (200/500 мс, 1–5 с, `CANCEL_BOUND`). Теперь эти границы
   идут по допущениям 3 и 4, а не остаются в обязательном прогоне.
7. **Opt-in профиль в CI не запускается.** Команда запуска описана в
   `DEVELOPMENT_RULES.md` (шаг 1).
8. **Фильтры CI не меняются.** Ни один тест, который целиком уходит в (б), в
   `ci.yml` не назван (команда A7, 0 совпадений). Тесты, названные в
   `run_exact` и `run_nonempty`, остаются функциями без `#[ignore]` и теряют
   только временные утверждения, поэтому каждый фильтр по-прежнему выполняет
   ≥ 1 тест.
9. **Числа строк — по базе `5b65205d`.** При реализации их сверяют по имени
   функции.

## Устройство сейчас

### 1. Два падения PR #444: точный лог

Лог — `gh run view <id> --log-failed`; в каждом run упал ровно один тест
(`grep -c '\.\.\. FAILED'` даёт 1 и 1).

**Run 38800079084, ФАКТ.**
`excluding_and_readmitting_a_provider_tombstones_and_restores_its_definition`
упал на `crates/bsl-analyzer/tests/shared_workspace_analysis_s6.rs:202:9` с
сообщением
`LSP definition failed: {"error":{"code":-32603,"message":"internal handler panic"},"id":4,"jsonrpc":"2.0"}`.

Ответ `internal handler panic` возникает только после реальной паники:
- `crates/bsl-analyzer/src/handlers/dispatch.rs:605` и
  `crates/bsl-analyzer/src/server.rs:1006` — при `panic_watch::panicked_in`;
- `dispatch.rs:678` — из `catch_unwind`.

Stderr сервера теряется: `crates/bsl-analyzer/tests/common/mod.rs:100` ставит
`Stdio::null()`. **Причина паники не установлена.** Связь с нагрузкой не
доказана, поэтому тест вне рамок (п. 3).

**Run 38812758876, ФАКТ.**
`g1_propagated_cancellation_from_live_writer_retries_only_after_new_revision`
упал на `crates/bsl-analyzer/src/lsp_live_vfs_propagated_tests.rs:570:13` с
сообщением `expected retry response`. В логе за 50 мкс до
`finalized LSP request request_id=RequestId(I32(731))` стоит
`publishing diagnostics uri=file:///tmp/.tmph2SOAf/Editor/Caller.bsl generation=1`.
Id 731 принадлежит только этому тесту (`:438`, `:490`), каталог `Editor` — его
фикстуре (`:135-137`).

Механизм:
1. Цикл `:532-545` обрабатывает посторонние задачи пула через `handle_task`.
2. Задача диагностики, запланированная после `Changed`, публикует
   `publishDiagnostics` в `client_receiver` раньше ответа.
3. `:566-571` читает первое сообщение и требует `Response`.

Класс — порядок сообщений. Причина записана в памяти pio.

### 2. Кандидаты разведки run 5328, сверенные с кодом

- **`crates/bsl-search/src/lifecycle.rs`: точные счётчики, ФАКТ.**
  - Сводка сбрасывается по размеру (`SUMMARY_FILES` = 128, `:17`) или по
    таймеру `summary.since.elapsed() >= Duration::from_secs(1)` (`:517-518`,
    `:591-592`); `flush` обнуляет интервальные поля (`:432-437`).
  - Точные числа записей зависят от того, сработал ли таймер:
    - `:723` — `1 + 2 * (10_000 / 128) + 1`;
    - `:756` — `== 4`;
    - `:783` — `started.len() == 3`;
    - `:689-691` — интервальные `files == 127`, `examples == 10`,
      `counts == 254`.
  - Тот же класс у `crates/bsl-search/src/engine/lifecycle_tests.rs:991-992`
    и `:1037` (`len <= 6 + 2 * elapsed_seconds`) и у
    `crates/bsl-search/src/store.rs:7439-7444`.
  - Класс подтверждён наблюдением: `vector_lifecycle_fused…` упал на Windows
    (run 38795855905, база `bb7576b3`), и `5b65205d` перевёл его на инвариант
    `started == 1 + progress`.
- **`crates/mcp-server/src/graph/build.rs`: фиксированные `sleep`, ФАКТ.**
  - `:2711` — `sleep(300)`, затем `read` ждёт `Busy` (`:2714`).
  - `:2808` — `sleep(200)`, затем окно `[1900, 3500)` мс (`:2813-2814`).
  - `:2775`, `:2780` — `sleep(150)`, затем `!is_finished()`.
  - `:2916` — `sleep(2 с)`, затем «ничего не собрано»: окно отсутствия,
    медленность его не роняет.
  - Пауза замены ждёт читателей `INSTALL_READERS_WAIT` = 5 с (`:1600`).
    Читатель ждёт допуска `ADMISSION_WAIT` = 2 с
    (`crates/mcp-server/src/graph/snapshot.rs:299`, `:374`) и получает `Busy`,
    только если на дедлайне всё ещё `held` (`:380`, `:463-468`).
  - Длительность паузы — параметр `pause_for_replacement(readers)`
    (`snapshot.rs:564`), но `publish_or_discard` передаёт константу
    (`build.rs:2141`).
- **Таймауты поиска, ФАКТ:** утверждения в `tools/search/*.rs` и
  `tools/query.rs:1009` (шаги 4–5).
- **`crates/parser/tests/integration_tests.rs`, ФАКТ.**
  - `test_small_file_performance` (`:609`, `avg_micros < 1000` на `:631`)
    содержит функциональную проверку `!has_errors()` (`:620`).
  - `test_lexer_performance_large_file` (`:635`, `elapsed < 500 мс` на
    `:653`) функциональных утверждений не содержит.
  - Тесты `:657`, `:829`, `:846`, `:868` время только печатают.

### 3. Граница: что считается зависимостью от нагрузки

| Класс | Определение | В границе |
|---|---|---|
| UPPER | `elapsed`/`waited` сравнивается с верхней границей < 10 с, или проверяется отношение двух замеров | да |
| WINDOW | окно `[низ, верх)`, верх < 10 с | да (верх) |
| SHORT | положительное ожидание события от другого потока или процесса со сроком < 10 с | да |
| SLEEP | `sleep` используется как синхронизация перед положительным утверждением | да |
| TIMER | точный счёт, который зависит от таймера продукта | да |
| ORDER | чтение «следующего сообщения» при конкурентном производителе других сообщений | да |
| SEEN | наблюдавшееся падение под нагрузкой с механизмом в тесте | да |
| LOWER | только нижняя граница; медленность её лишь удлиняет | нет |
| NEG | окно отсутствия (`!recv_timeout`, `NotYet`, `expect_err`); медленность не роняет | нет |
| WATCH | срок ≥ 10 с только против зависания | нет |
| PACE | `sleep` как шаг опроса внутри цикла с потолком, разведение mtime, пауза перед действием | нет |
| SYNC | событие уже в очереди: тот же поток, синхронный `notify` | нет |

Входные множества по базе. Команды — в Приложении А, числа — их вывод.

- A1: сравнения времени в утверждениях — 156, из них 93 без сторожей циклов
  `now() < deadline` (A1f);
- A2: ожидания с литералом < 10 с — 82;
- A3: `sleep` — 421, из них 87 в `diagnostics_state/drift.rs`;
- A10: вычисляемые дедлайны < 10 с — 28;
- A11: константы `Duration` < 10 с — 28.

Места A1–A3 разобраны по пяти группам (128 + 198 + 167 + 52 + 107 строк,
каждая с `файл:строка`), A10 и A11 — в этом плане. Подмножество в границе —
тесты шагов 2–7, остальное — Приложение Б.

Граница шире кандидатов run 5328, потому что требование касается всего
обязательного прогона, а те же классы сканеры находят во всех крейтах.

ГИПОТЕЗА: любой тест границы может упасть на корректном коде при задержке
выше своего запаса. Наблюдались только тесты SEEN (шаг 6) и g1 (шаг 2).

### 4. Существующие средства, на которые опираются решения

- `recv_client_response(receiver, id)` пропускает уведомления до ответа с
  нужным id, сторож — `CLIENT_RESPONSE_HANG_GUARD` = 10 с
  (`crates/bsl-analyzer/src/server.rs:2249`, `:2296-2300`). Введён в `0cd1a057`.
- `wait_settled` ждёт проход публикации, свободный слот и `!walk_out()`
  (`crates/mcp-server/src/graph/test_support.rs:222-228`). `wait_ready` — только
  барьер статуса (`:200-218`).
- `hold_lifecycle_lock_for_test` держит мьютекс `checked_at`
  (`crates/mcp-server/src/workspace_lease.rs:1054-1057`). Его берёт
  `owns_caches` до проверки по файлу (`:601`). Heartbeat вызывает
  `owns_caches()` каждую секунду (`:1209`). Вердикт сброшен, а запись lease
  удалена, поэтому `recheck_locked` пытается захватить удержанный лок и
  возвращает `None` (`:1142-1157`). `settle` публикует `Unknown` (`:758-770`), а
  наблюдатель графа ставит `Admission::Paused`
  (`crates/mcp-server/src/graph/state.rs:1353-1354`).
- `Patience`/`eventually_within` (`crates/mcp-server/src/change_hub.rs:5813`,
  `:5859`), `wait_until` (`graph/test_support.rs:136`, потолок `WAIT_CEILING`), `TestClock`
  (`crates/mcp-server/src/state/overlay_backlog.rs:641`),
  `with_retry_pacing_for_test` (`crates/mcp-server/src/diagnostics_state/lifecycle.rs:512`),
  `engine.queued()` (`crates/mcp-server/src/tools/search/acquire.rs:269`).

## Шаги

Обозначения решений:
- **В** — срок положительного ожидания заменяется общим потолком крейта
  (допущение 2).
- **Т** — временное утверждение переносится под
  `if cfg!(timing_contract) { … }` (допущение 4).
- **Б/2** — граница `B / 2` от продуктового бюджета B ≥ 20 с (допущение 3).
- **Н** — синхронизация по наблюдаемому состоянию вместо `sleep` или времени.
- **И** — инвариант вместо точного счёта.
- **П** — чтение до ответа с нужным id.
- **O** — тест целиком: `#[cfg_attr(not(timing_contract), ignore = "timing contract: …")]`.

«CI» — строка `ci.yml`, где тест назван явно; прочерк значит,
что тест запускается только через `cargo test --all` (`:312`).

### Шаг 1. Профиль `timing_contract`

- `Cargo.toml:20`: `check-cfg = ['cfg(corpus_contract)', 'cfg(timing_contract)']`.
- `docs/contributing/DEVELOPMENT_RULES.md` — рядом с «Тесты и корпус» (`:279`)
  раздел «Тесты и время»:
  - что попадает в профиль;
  - правило: в обычном профиле нет временных верхних границ < 10 с;
  - команда запуска.

  ```bash
  CARGO_TARGET_DIR=target-timing RUSTFLAGS="--cfg timing_contract" cargo test --workspace
  ```

Результат: `cargo clippy --all-targets --all-features -- -D warnings` проходит.
Без cfg тесты O видны как `ignored`, с cfg — как обычные.

### Шаг 2. Порядок сообщений (П)

| Тест | Место | Класс | Решение |
|---|---|---|---|
| `g1_propagated_cancellation_from_live_writer_retries_only_after_new_revision` | `crates/bsl-analyzer/src/lsp_live_vfs_propagated_tests.rs:395`, чтение `:566-571` | ORDER, SEEN | П: `recv_client_response(&client_receiver, request_id)` |
| `g1_propagated_cancellation_from_real_query_panic_is_terminal_internal_error` | `:609`, чтение `:687-692` | ORDER (та же форма чтения) | П |
| `g1_propagated_without_new_revision_is_refused_controller_control` | `:722`, чтение `:767-772` | ORDER (та же форма чтения) | П |

Результат: три теста проходят, даже если перед ответом в канале стоит
уведомление (инвариант И2).

### Шаг 3. Счёт сводок по инварианту (И)

Инвариант сводок одной операции из N мутаций при любом числе срабатываний
таймера:
- `started = 1 + pending_вызовов + progress`;
- у каждой записи `files ≤ SUMMARY_FILES`;
- Σ`files` по не-`started` = N, поэтому `progress ≥ ⌈N/128⌉ − 1`;
- каждая `started` без интервальных данных.

| Тест | Место | Класс | Решение |
|---|---|---|---|
| `subscriber_gets_bounded_summaries_and_terminal_committed_totals` | `crates/bsl-search/src/lifecycle.rs:703`, `:723` | TIMER | И; точное `1 + 2*78 + 1` — Т |
| `unavailable_totals_keep_their_quality_after_flush_and_at_termination` | `crates/bsl-search/src/lifecycle.rs:743`, `:756`, `:761-763` | TIMER | И: первая `started`, последняя `cancelled`, за каждой `progress` идёт `started`, Σ`files` = 128; `len == 4` и индексы — Т |
| `started_intents_carry_no_interval_data` | `crates/bsl-search/src/lifecycle.rs:767`, `:783` | TIMER | И: `started = 2 + progress`; `== 3` — Т |
| `aggregation_and_explicit_worker_context` | `crates/bsl-search/src/lifecycle.rs:670`, `:689-691` | TIMER | И: `totals.sqlite_vectors_removed == Some(254)` (`totals` не сбрасывается, `:420`); Σ`files` выпущенных `progress` плюс текущий интервал = 127; интервальные `== 127/10/254` — Т |
| `vector_lifecycle_context_and_mode_decisions_preserve_exact_loss` | `crates/bsl-search/src/engine/lifecycle_tests.rs:963`, `:991-992` | TIMER | И: сумма `reasons.context_changed` и `files` по сводкам `context_changed` = 2; поиск единственной `completed` — Т |
| `vector_lifecycle_bulk_removal_bounds_info_and_retains_commits_before_refusal` | `engine/lifecycle_tests.rs:1005`, `:1037` | TIMER | И: `len == 2 + 2·progress`, Σ`files` = removed; `len <= 6 + 2·elapsed_seconds` — Т |
| `overlay_cache_save_bounds_info_and_retains_prior_autocommits` | `crates/bsl-search/src/store.rs:7432`, `:7439-7444` | TIMER | И: `started = 1 + progress`, Σ`files` = 129; `len == 4`, `intents == 2` — Т |

Положительный контроль инварианта — новый тест в `lifecycle.rs`: между
мутациями одного интервала `sleep(1100 мс)`. Это нижняя граница, поэтому
таймер сработает гарантированно. Новый тест проверяет, что старое точное
утверждение на таком входе ложно, а инвариант истинен.

Результат: ни одно утверждение шага в обычном профиле не зависит от срабатывания
таймера.

### Шаг 4. Ожидания и синхронизация (В, Н)

**bsl-analyzer**

| Тест | Место | Класс | Решение |
|---|---|---|---|
| `g1_workspace_diagnostic_real_partial_is_terminal_after_pending_write` | `crates/bsl-analyzer/src/server.rs:4545`, `:4758` (50 мс), дедлайн `:4751` (10 с) | SHORT | В: дедлайн `:4751` = `HANG_WATCHDOG`, `recv_timeout(deadline − now)` |
| `bounded_pool_executes_more_jobs_than_workers` | `crates/bsl-analyzer/src/task_pool.rs:199`, `:222`, `:232` | SHORT | В |
| `try_spawn_reports_full_and_recovers` | `task_pool.rs:291`, `:322` | SHORT | В |
| `panicking_job_does_not_kill_pool` | `task_pool.rs:341`, `:350` | SHORT | В |
| `on_latency_happy_path`, `on_latency_panic_becomes_internal_error`, `on_latency_duplicate_id_cancels_previous`, `late_cancel_after_completion_is_noop` | `crates/bsl-analyzer/src/handlers/dispatch.rs`: ожидания 5 с на `:783`, `:810`, `:844`, `:1120` (последний тест объявлен на `:1101`) | SHORT | В |
| `vector_journal_failure_keeps_stdout_protocol_only` | `crates/bsl-analyzer/tests/vector_journal_cli.rs:91` | SHORT | В. CI `:311` |
| `watched_bsl_remove_and_recreate_updates_both_protocols` | `crates/bsl-analyzer/tests/shared_workspace_analysis.rs:425`, `:450`, `:478` | SLEEP (8 × 25 мс) | Н: цикл до условия с дедлайном файла (как `:55-68`) |
| `source_set_change_retires_mcp_scope_and_reconnects_to_new_semantics` | `crates/bsl-analyzer/tests/shared_workspace_analysis_s6.rs:260`, ожидание `:367` | SHORT (5 с) | В (`HANG_WATCHDOG`) |
| `invalid_project_config_preserves_lsp_and_rejects_mcp_until_repair`, `unknown_and_excluded_rooted_paths_do_not_collide` | `crates/bsl-analyzer/tests/shared_workspace_analysis_s10.rs:358` (ожидание `:479`), `:492` (ожидание `:650`) | SHORT | В |
| `McpSession::shutdown` (все `workspace_cache_scope_*`) | `crates/bsl-analyzer/tests/source_set_cli.rs:4089-4091` | SHORT (5 с, затем kill и Err) | В. CI `:36`, `:367` |
| `workspace_cache_scope_http_cold_and_warm_runs_keep_sources_clean` | `source_set_cli.rs:1960`; `wait_for_cache_adoption` `:103-112` | SHORT | В. CI `:36`, `:367` |
| `workspace_cache_scope_s12_proxy_parent_child_composition_mismatch` | `source_set_cli.rs:2870`, драйвер `s12_proxy_scope_mismatch_driver` `:3062-3073` | SHORT | В. CI `:36`, `:367` |
| `call_hierarchy_index_single_worker_no_deadlock` | `crates/bsl-analyzer/src/handlers/request.rs:3126`, дедлайны `:3161` (1 с), `:3176` (2 с) | SHORT | В |
| `call_hierarchy_index_lsp_cancelled_wait_releases_permit_without_cancelling_build` | `request.rs:3303`, дедлайн `:3324` (100 мс) | SHORT | В |
| помощник `wait_for_scope_guard_exit` и помощник `exercise` теста `workspace_cache_scope_http_cold_and_warm_runs_keep_sources_clean` | `source_set_cli.rs:876`, `:908`; `:1974`, `:2155` (2 с до снятия lease) | SHORT | В. CI `:36`, `:367` |
| `workspace_cache_scope_optional_journal_overlap_keeps_source_clean` | `source_set_cli.rs:2238`, `:2326` | SHORT | Н: `join` потока stderr после shutdown, затем снимок, как `:898-902` |
| `workspace_cache_scope_daemon_broker_and_broker_required_share_only_the_leaf` | `source_set_cli.rs:2650`, `:2717` | SHORT | В. CI `:36`, `:367` |

**mcp-server: change_hub, state, lease, lib, baseline**

| Тест | Место | Класс | Решение |
|---|---|---|---|
| `a_hub_still_starting_says_not_yet_and_arms_once_released` | `crates/mcp-server/src/change_hub.rs:6661`, `:6671` | SHORT | В |
| помощник `polling_hub` и его тесты | `change_hub.rs:8224`, `:8231` | SHORT | В |
| `shutdown_stops_the_fallback_poll` | `change_hub.rs:8505`, `:8513` | SHORT | В. CI `:134`, `:662` |
| `superseded_check_never_waits_for_the_lease_lifecycle_lock` | `crates/mcp-server/src/state/mod.rs:1494`, `:1504` | SHORT (лок снимается после ответа) | В |
| `an_owner_told_to_leave_answers_stopping_even_with_the_engine_held` | `state/mod.rs:1383`, `:1399` | SHORT | В. CI `:142`, `:667` |
| `workspace_cache_scope_shutdown_waits_for_owners_before_releasing_lease` | `state/mod.rs:1305`, `:1333` | SHORT | В. CI `:34` (префикс) |
| `last_owner_exit_cannot_notify_between_empty_check_and_wait` | `state/mod.rs:1270-1287`, `:1277` | SHORT | В |
| `shutdown_stops_every_owner_within_a_second` | `state/mod.rs:1703`, `:1714` | SHORT | В (`eventually` с `STATE_CEILING`). CI `:82`, `:611` |
| `shutdown_does_not_wait_out_an_engine_hold` | `state/mod.rs:1723`, `:1747` | SHORT | В. CI `:122`, `:650` |
| `shutdown_stops_a_polling_hub` | `state/mod.rs:1758`, `:1787` | SHORT | В. CI `:148`, `:673` |
| `shutdown_stops_the_poll_of_a_blind_root` | `state/mod.rs:1799`, `:1835` | SHORT | В. CI `:184` |
| `a_key_that_keeps_failing_is_retried_on_the_schedule` | `crates/mcp-server/src/state/overlay_backlog.rs:1800`, `:1809-1810` | SLEEP, TIMER | Н: `TestClock` через `probe.clock` без `advance`, как `:967` |
| `temporary_lock_refusal_remains_retryable` | `crates/mcp-server/src/state/overlay_retry.rs:1378`, `:1386` | SLEEP | Н: `wait_for` по `pass_count` (`:941`) |
| `concurrent_kicks_collapse_into_one_pass` | `overlay_retry.rs:1297` | SLEEP | Н: ждать `pass_count ≥ 1` и отсутствия запланированного прохода, затем `== 1` |
| `a_request_coalesce_does_not_erase_the_backoff_of_a_running_pass` | `overlay_retry.rs:817` | WINDOW | Н: `at = Instant::now()` до `settle_outcome`; `next_allowed ≥ at + retry_delay(1)` |
| `the_boot_keeps_waiting_while_the_hub_is_still_starting` | `crates/mcp-server/src/state/sync.rs:3088`, `:3100`, `:3111` | SLEEP (нижняя граница от позднего `started`) | Н: `started` до `spawn` |
| `workspace_cache_scope_config_change_stops_owner_transport_but_body_drift_stays_hot` | `crates/mcp-server/src/state/bootstrap.rs:5099` | SHORT | В (`wait_empty(STATE_CEILING)`). CI `:34` |
| `workspace_cache_scope_change_invalidates_frozen_extension_scope` | `bootstrap.rs:5105`; `:5181`, `:5217`, `:5254`, `:5289` | SHORT | В. CI `:34` |
| `reference_search_keeps_pending_external_baseline_loading_until_shutdown` | `bootstrap.rs:5319`, `:5336` | SHORT | В |
| `reference_search_does_not_fall_back_for_unavailable_postgres_intent` | `bootstrap.rs:5378`, `:5374` | SHORT (100 × 10 мс) | В (`Patience`) |
| `workspace_cache_scope_default_lease_denial_blocks_search_and_paid_vector_work` | `crates/mcp-server/src/state/embed.rs:4900` | SHORT | В. CI `:34` |
| `production_root_validation_runs_off_the_engine_mutex`, `event_across_root_validation_keeps_the_old_table_and_retry_obligation`, ожидание графа на `:2382` | `embed.rs:2631`, `:2691`, `:2382` | SHORT (≈ 4 с) | В |
| помощник `wait_for_root_count` | `embed.rs:2244-2245` | SHORT | В |
| `stale_snapshot_served_during_loading` | `crates/mcp-server/src/lib.rs:4716-4739` | SHORT | В |
| константа `SHUTDOWN_TIMEOUT` = 5 с и 20 её мест (помощник `session_finished` и тесты файла) | `crates/mcp-server/src/serve_stream_tests.rs:16`, `:66-70` | SHORT | В: константа = `STATE_CEILING` |
| `all_status_requests_are_cached_under_held_lease` | `crates/mcp-server/src/lib.rs:3621`, `HANG_GUARD` 500 мс `:3618` (места `:3638`, `:3662`, `:3687`) | SHORT (лок снимается после проверки журнала `:3717`) | В; 500 мс — Т |
| `an_unestablished_ownership_verdict_is_not_published_as_one` | `crates/mcp-server/src/lib.rs:4163`, `:4204` (500 мс) | SHORT (лок снимается после ответа) | В |
| `a_status_answer_does_not_queue_behind_a_background_ownership_check` | `crates/mcp-server/src/lib.rs:4254`, `:4295` | SHORT | В |
| `deferred_slot_pending_then_publish_wakes_waiters` | `crates/mcp-server/src/baseline.rs:2140`, `:2149` | SHORT | В |
| `external_baseline_service_shutdown_times_out_without_blocking_future_requests` | `baseline.rs:2810` | SHORT | В |

**mcp-server: graph, tools, tasks, tests**

| Тест | Место | Класс | Решение |
|---|---|---|---|
| `try_acquire_engine_queues_until_the_lock_frees` | `crates/mcp-server/src/tools/search/acquire.rs:513`, `:534-540` | SLEEP | Н: ждать `engine.queued() == 1` (`:269`), до `drop` — `!probe.is_finished()`; `waited ≥ HOLD/2` — Т |
| `status_tells_an_unread_root_table_from_a_missing_one` | `crates/mcp-server/src/tools/search/status.rs:1288`, `:1339` | SLEEP | Н: держатель отпускает лок по каналу после возврата `search_status_with_cap` |
| `the_body_observes_the_request_token` | `crates/mcp-server/src/tools/search/call.rs:98`, `:114-115` | SLEEP | Н: канал из тела после `store`, затем `recv_timeout` с потолком |
| `a_waiting_task_counts_as_use_of_the_resident` | `crates/mcp-server/src/tasks.rs:918`, `:922`, `:935`, `:939-943` | UPPER (сравнение возрастов) | Н: опрос до `STATE_CEILING`, пока `access_age()` меньше времени с запуска (`mark_in_use` на каждом проходе, `tasks.rs:413`); сравнение — Т |
| `a_failed_graph_is_retried_when_its_backoff_runs_out` | `crates/mcp-server/src/graph/watcher.rs:839`, `:847-851` | UPPER (запас 400 мс до `due`) | Т для `:847-851`; нижняя граница `:855` остаётся |
| `a_body_edit_reloads_the_graph_with_no_other_consumer` | `watcher.rs:1055`, `:1076-1081` | TIMER (debounce 150 мс / 1 с, `:29-30`) | И: `revision > before`, `full_builds` не растут; точный счёт — Т |
| `superseded_daemon_lifecycle` | `crates/mcp-server/tests/superseded_daemon_lifecycle.rs`: `wait_superseded` `:46-55`, инициализация хранилища `:156-161`, `wait_current_graph` `:31-43` | SHORT (5–10 с) | В: в файл добавляется `mod common;`, ожидания — `common::Patience` с `common::STATE_CEILING` (`crates/mcp-server/tests/common/mod.rs:20`, `:23`) |

**остальные крейты**

| Тест | Место | Класс | Решение |
|---|---|---|---|
| `two_stops_queued_between_waits_are_both_reported_latest_first` | `crates/bsl-debug/src/session.rs:1625`, `:1649` → `sleep(200)` в `wait_until` (`:1721`) | SLEEP | Н: опрос `stopped_count() == 2` (`:1653`), потолок В |
| `a_real_stop_replaces_the_halted_before_attach_announcement` | `crates/bsl-debug/src/session.rs:1727`, `:1739-1741` | SLEEP | Н: `wait_for_stop(потолок)` до причины ≠ `Suspended`; окно 50 мс (`:1744`) — NEG, остаётся |
| `locals_without_an_answer_is_an_error_not_an_empty_frame` | `crates/bsl-debug/src/session.rs:2008`, `:2023-2024` | SLEEP, SHORT | В: ожидание остановки с потолком, без `wait_until`-сна |
| `a_message_only_stop_whose_continue_is_refused_does_not_fail_the_wait` | `crates/bsl-debug/src/session.rs:2034`, `:2056`, `:2060-2063` | SLEEP | Н: опрос `take_runtime_warnings()` с накоплением, потолок В |
| `a_refused_event_channel_is_reported` | `crates/bsl-debug/src/session.rs:1985`, `:1995` | SHORT | В |
| `a_contended_fenced_open_never_spans_the_contention_in_one_admission` | `crates/bsl-search/src/engine.rs:12577`, `:12594` | SHORT (5 с) | В |
| `embedding_publish_fence` | `crates/bsl-search/src/engine.rs:7442`, `:7497`, `:7632-7635` | SLEEP (50 мс держит параллельность) | Н: обработчик ждёт `active == 2` с потолком, потом отвечает. CI `:528` |
| `payload_shared_publish_preserves_worker_concurrency_bound` | `crates/bsl-search/src/publish.rs:990`, `:1010`, `:1037` | SHORT | В |
| `context_observer_matches_original_sql_with_competing_writer_and_rollback` | `crates/bsl-search/src/store.rs:7807`, `:7819`, `:7892` | SHORT | В |
| `a_fifo_in_the_managed_directory_does_not_block_the_reader` | `crates/project-model/src/diagnostics_baseline_fs.rs:367`, `:381` | SHORT | В |
| `a_fifo_named_configuration_xml_does_not_block_the_scan` | `crates/project-model/src/lib.rs:5531`, `:5552` | SHORT | В |
| `a_future_hard_exclusion_narrows_ancestors_and_is_never_watched` | `crates/vfs-notify/src/lib.rs:1816` | SHORT | В |
| `an_excluded_v8_import_directory_can_be_deleted_and_recreated_on_windows` | `vfs-notify/src/lib.rs:1866`, `:1891` | SHORT | В |

**mcp-server: `diagnostics_state`.** Ручные циклы опроса 2–3 с ждут
асинхронную перезагрузку (`bsl-diag-reload`) или доставку наблюдателя. Все
заменяются на `wait_until` (`crates/mcp-server/src/diagnostics_state/test_support.rs:188`, бюджет 30 с — `:175-180`).

| Тест (`drift::tests::…`) | Место в `drift.rs` |
|---|---|
| `config_edit_triggers_reload_with_new_config` | `:1618`, `:1640-1648` |
| `analyzer_config_edit_still_full_rebuilds` | `:2692`, `:2712-2720` |
| `symlinked_subtree_outside_root_xml_edit_is_not_lost` | `:2771`, `:2797-2804` |
| `event_driven_config_edit_full_rebuilds` | `:3185`, `:3203-3211` |
| `events_after_rebuild_apply_to_the_new_resident` | `:3218`, `:3237-3245` |
| `reconciler_does_not_degrade_a_late_delivered_edit` | `:3614`, проба `:3635-3642` |
| `a_path_delivered_alongside_a_rescan_is_not_a_miss` | `:3757`, проба `:3780-3787` |
| `a_scan_taken_before_a_rebuild_is_not_applied_to_the_new_resident` | `:3954`, `:3967-3972` |
| `a_read_applies_the_entries_delivered_with_a_rescan_demand` | `:4142`, проба `:4168-4176` |
| `an_event_delivered_after_the_scan_is_applied_in_the_same_tick` | `:4220`, проба `:4246-4254` |
| `incomplete_scan::a_config_edit_does_not_rebuild_over_a_tree_it_cannot_read` | `:4765`, `:4810-4818` |
| `incomplete_scan::a_delivered_config_edit_does_not_rebuild_over_a_tree_it_cannot_read` | `:4879`, `:4928-4936` |
| `incomplete_scan::a_declined_rebuild_does_not_swallow_the_events_it_consumed` | `:4947`, `:4977-4989`, `:5003-5010` |
| `incomplete_scan::a_pending_config_edit_admits_no_new_files` | `:5060`, `:5100-5107` |
| `incomplete_scan::a_pending_config_edit_admits_no_new_files_through_the_drain` | `:5116`, предусловие `:5141-5144`, `:5171-5179` |
| `incomplete_scan::a_pending_config_edit_admits_no_new_descriptors_through_the_drain` | `:5188`, предусловие `:5212-5215`, `:5237-5245` |

Ещё в `diagnostics_state`:

| Тест | Место | Класс | Решение |
|---|---|---|---|
| `the_retry_wait_counts_down` | `crates/mcp-server/src/diagnostics_state/lifecycle.rs:2045`, `:2054` (база 5 с) | UPPER | Н: `with_retry_pacing_for_test(3600 с, 3600 с)` (`:512`). `sleep(80)` (`:2052`) остаётся: он даёт нижнюю границу `first − second ≥ 40` (`:2056-2057`), а медленность её только увеличивает |
| `a_loading_build_that_overruns_its_deadline_is_published_failed` | `crates/mcp-server/src/diagnostics_state/lifecycle.rs:1872`, `:1897` | UPPER | Н: то же |
| `a_failed_build_is_retried_after_its_backoff_and_then_succeeds` | `crates/mcp-server/src/diagnostics_state/lifecycle.rs:1770`, `:1784` (база 60 мс) | UPPER | Т для `owed > 0`; то же свойство при базе 3600 с проверяет `a_retry_before_its_backoff_starts_no_build` (`:1832`) |
| `a_cancel_arriving_during_the_wait_stops_the_retry` | `crates/mcp-server/src/diagnostics_state/session.rs:743`, тикер `:757-766` | WINDOW | Н: отмена из `set_pre_force_probe` (`drift.rs:103`) |
| `a_cancelled_call_frees_the_resident_slot` | `crates/mcp-server/src/diagnostics_state/session.rs:369`, `:418` (отношение) | UPPER | Н: счётчик `walk_probe::entered() < CALLERS` в момент ответа соседа; отношение — Т |

Результат шага: срок каждого перечисленного положительного ожидания — общий
потолок, и ни одно положительное утверждение не стоит за `sleep`.

### Шаг 5. Верхние границы времени (Б/2, Т, Н)

| Тест | Место | Бюджет, который ловит граница | Решение |
|---|---|---|---|
| `a_refused_journal_neither_spins_nor_delays_shutdown` | `crates/bsl-analyzer/src/bin/cli/logging/journal.rs:705`, `:738` | `DRAIN` = 2 с (`:27`) | Т. CI `:310`, `:391` (фильтр `cli::logging`; тест остаётся) |
| `call_hierarchy_index_lsp_timeout_returns_none_then_serves_published_index` | `crates/bsl-analyzer/src/handlers/request.rs:3268`, `:3292` | умолчание 2 с | Т |
| `a_wait_an_idle_server_will_never_satisfy_is_judged_lost_not_timed_out` | `crates/bsl-analyzer/tests/diagnostics_baseline_lsp.rs:177`, `:196` | окно 2 с | Н: сообщение несёт окно (`tests/common/mod.rs:239-240`) — проверить `contains` его; `waited < SILENCE` — Т |
| `a_continuous_unrelated_message_stream_cannot_extend_a_wait_past_its_ceiling` | `diagnostics_baseline_lsp.rs:203`, `:235` | `stream_for` | Т для `waited < stream_for`; текст «total deadline elapsed» уже проверяется на `:230-232` и остаётся |
| `a_hub_whose_thread_never_started_reports_failure_at_once` | `crates/mcp-server/src/change_hub.rs:6643`, `:6653` | 30 с (`:6652`) | Б/2: `< 15 с` |
| `a_blind_poll_parked_before_its_first_reading_stops_with_the_hub` | `change_hub.rs:9494`, `:9507` | `STOP_BUDGET` = 5 с (`:48`) | Т; счётчик `eventually_within` (`:9504`) остаётся |
| `interrupted_waiters_return_at_once_and_stay_released` | `change_hub.rs:9895`, `:9909`, `:9913`, `:9916` | ожидания по 60 с (`:9903`, `:9912`, `:9915`) | Б/2: `< 30 с` во всех трёх местах; исходные `< 5 с`/`< 1 с` — Т. CI `:83`, `:612` |
| `a_released_hold_lets_a_parked_hub_stop_at_once` | `change_hub.rs:10683`, `:10694` | `STOP_BUDGET` | Т |
| `an_explicit_shutdown_before_the_last_drop_is_harmless` | `change_hub.rs:10702`, `:10712` | `STOP_BUDGET` | Т |
| `a_stop_releases_an_owner_asleep_in_a_hub_wait` | `crates/mcp-server/src/state/mod.rs:1845`, `:1889` | 30 с (`:1879`) | Б/2: `recv_timeout(15 с)`. CI `:149`, `:674` |
| `a_large_backlog_drains_without_pauses_between_batches` | `crates/mcp-server/src/state/overlay_backlog.rs:1456`, `:1476` | `idle_tick` 30 с (`:1460`) | Б/2: `widest < idle_tick / 2`; `:1472` остаётся. CI `:95`, `:623` |
| `a_request_takes_the_engine_while_a_batch_is_being_prepared` | `overlay_backlog.rs:1818`, `:1859` (сканер приписывает её методу мока `text_and_parse`) | `ACQUIRE_POLL` 25 мс | Н: вызов в потоке; до `release_tx.send` (`:1857`) фаза B припаркована, ответ пришёл; сравнение — Т |
| `a_held_writer_lock_refuses_the_commit_and_a_stop_is_answered_during_the_backoff` | `overlay_backlog.rs:1867`, `:1883` | backoff | Т; счётчик `:1882` остаётся. CI `:96`, `:624` |
| `a_paused_driver_leaves_in_every_shutdown_order` | `crates/mcp-server/src/state/overlay_retry.rs:1405`, `:1521` | `TICK` = 30 с (`:26`) | Б/2: `< TICK / 2`. CI `:144`, `:669` |
| `the_published_verdict_is_readable_while_a_check_holds_the_lifecycle_lock` | `crates/mcp-server/src/workspace_lease.rs:1535`, `:1563` | I/O проверки | Н: `hold_cache_lock_until_released`; чтение в потоке до ответа под потолком, пока лок держится; `waited < 100 мс` — Т |
| `the_restamp_window_spans_separate_fences` | `crates/mcp-server/src/workspace_lease.rs:2232`, `:2247` | `CHECKPOINT_MIN_INTERVAL` = 1 с (`:74`) | Т (окно < 10 с, хука часов нет) |
| `reading_every_stored_key_does_not_hold_the_engine_lock` | `crates/mcp-server/src/state/sync.rs:1632-1633` | `ACQUIRE_POLL` | Н: до `release_tx.send` (`:1628`) `!preparer.is_finished()` и захват получен; сравнение — Т |
| `the_wait_never_overshoots_its_budget_by_a_slice` | `sync.rs:3152`, `:3165`, `:3171` | слайс, который тест передаёт `WatchWaitPolicy::new` | Б/2: слайс 400 мс → 30 с (B), бюджет 20 мс, граница 200 мс → `< 15 с` |
| `the_boot_does_not_wait_out_a_permanent_failure` | `sync.rs:3179`, `:3195` | 600 с (`:3192`) | В: граница `STATE_CEILING` < 600 с остаётся детектором |
| `a_request_does_not_wait_on_a_held_lease_lock` | `crates/mcp-server/src/lib.rs:4504`, `:4532` | — | В для завершения; журнал `:4537` остаётся; `500 мс` — Т |
| `every_workspace_search_cell_answers_a_cancelled_request_before_the_lock_frees` | `crates/mcp-server/src/lib.rs:5962`, `assert_cancelled` `:5955`, `BOUND` `:5945` | лок держится до ответа | Т; сторож `STATE_CEILING` (`:5983`) остаётся |
| `status_probe_first_call_returns_pending_without_blocking` | `crates/mcp-server/src/baseline.rs:2929`, `:2935` | — | Т; исход `Pending` (`:2944`) остаётся |
| `a_withdrawn_caller_stops_waiting_and_its_queued_request_is_skipped` | `baseline.rs:3051`, `:3108`, `:3112` | — | Н: `!first.is_finished()` перед `release_tx.send`; `:3108` — В; `:3112` — Т |
| `deferred_slot_shutdown_wakes_a_pending_waiter` | `baseline.rs:2166`, `:2176` | бюджет ожидающего 5 с (`:2171`) | В для `recv`; `200 мс` — Т |
| `a_patch_does_not_wait_long_for_another_writer` | `crates/mcp-server/src/graph/build.rs:4022`, `:4041` | `PATCH_LOCK_WAIT` 250 мс | Т; исход `Busy` (`:4040`) остаётся |
| `a_request_miss_never_consults_the_lease` | `crates/mcp-server/src/graph/snapshot.rs:2319`, `:2367` | — | Т; журнал дисковых проверок (`:2359-2366`) остаётся |
| `background_snapshot_preserves_all_typed_outcomes` | `crates/mcp-server/src/graph/snapshot.rs:2454`, `:2486` | — | Т; журнал `:2479-2485` остаётся |
| `assert_all_search_modes_are_resident_only_under_held_lease` | `crates/mcp-server/src/tools/search/hybrid.rs:236`, `:294` | — | Т; журнал `:289-293` остаётся |
| `acquire_engine_reports_poison_immediately` | `crates/mcp-server/src/tools/search/acquire.rs:563`, `:582` | cap 30 с (`:328`) | Б/2: `< 15 с` |
| `acquire_engine_stops_waiting_when_the_request_is_cancelled` | `acquire.rs:589`, `:616` | cap 2 с | Т |
| `a_search_waiting_for_the_engine_lock_returns_at_its_cancellation` | `crates/mcp-server/src/tools/search/cancel_tests.rs:121`, `:146`, контроль `:154-159` | `HOLD` | Н для контроля: `queued() == 1` вместо `sleep(HOLD)`; `:146` и `waited ≥ HOLD/2` — Т |
| `a_cancelled_search_waiting_on_the_actor_releases_the_engine_lock` | `cancel_tests.rs:166`, `:196`, `:203` | `CANCEL_BOUND` | Т |
| `a_cancelled_identity_gate_releases_the_engine_lock` | `cancel_tests.rs:213`, `:244`, `:249` | `CANCEL_BOUND` | Т |
| `a_cancelled_docs_search_does_not_fall_back_to_the_corpus_view` | `cancel_tests.rs:260`, `:280` | `CANCEL_BOUND` | Т |
| `a_search_inside_the_query_embed_holds_no_lock_and_returns_at_its_cancellation` | `cancel_tests.rs:332`, `:384` | `CANCEL_BOUND` | Т |
| `search_status_returns_promptly_with_busy_note_when_engine_lock_is_held` | `crates/mcp-server/src/tools/search/status.rs:1981`, `:1992`, `:2019` | — | Н: держатель отпускает лок по каналу после возврата; вызов в потоке с потолком; `< 2 с` — Т |
| `search_status_reports_pending_probe_without_blocking` | `crates/mcp-server/src/tools/search/status.rs:1751`, `:1775` | — | Т |
| `a_cancelled_request_stops_waiting_for_a_silent_producer` | `crates/mcp-server/src/tools/search/wait.rs:106`, `:122` | — | Т |
| `rendering_cost_stays_proportional_to_the_number_of_findings` | `crates/mcp-server/src/tools/query.rs:983`, `:1009` | замер производительности | Т для границы `:1009`; утверждение о размере фикстуры (`:990`) остаётся |
| `cancelling_a_working_handle_stops_the_read_and_frees_the_resident` | `crates/mcp-server/tests/tasks.rs:386`, `:396` (отношение) | — | Т |
| `superseded_backend_lifecycle` | `crates/mcp-server/tests/broker.rs:247`, `:337-338` | `idle_ttl` 10 с, запас 5 с | Т для окна «ещё жив через 5 с». CI `:531` (тест остаётся) |
| `a_shutdown_wakes_the_idle_sweeper_out_of_its_tick` | `crates/mcp-server/src/diagnostics_state/lifecycle.rs:1576`, `:1600`, `:1603` | `SWEEP_INTERVAL` = 30 с (`:25`) | Б/2: `< SWEEP_INTERVAL / 2`; `:1600` — В. CI `:150`, `:675` |
| `a_zero_wait_reports_a_halt_before_the_attach_without_waiting_for_its_stack` | `crates/bsl-debug/src/session.rs:1836`, `:1839`, `:1848` | фейковый стек 3 с | Н: обработчик `getCallStack` ждёт канал, который тест открывает после проверки; `wait_for_stop(ZERO)` в потоке с потолком; `< 500 мс` — Т |
| `a_busy_store_gives_the_batch_back_and_the_retry_reads_nothing` | `crates/bsl-search/src/point_refresh.rs:629`, `:645` | `C_BUSY_TIMEOUT` 100 мс против `DEFAULT_BUSY_TIMEOUT` 30 с (`store.rs:121`) | Б/2: верх `< 15 с`; низ — LOWER. CI `:129`, `:657` |
| `note_stamps_position_and_resets_stall_clock` | `crates/ide/src/graph.rs:3175`, `:3177-3179` | — | Н: `before = ms_since_progress()` после `sleep(30)`, затем `after < before`; `< 25` — Т |
| `call_hierarchy_reverse_index_insert_pair_hot_target` | `crates/hir-def/src/call_hierarchy_index/tests.rs:97`, `:114` | производительность | Т; функциональные `:118-122` остаются |
| `test_small_file_performance` | `crates/parser/tests/integration_tests.rs:609`, `:631` | производительность | Т; `!has_errors()` (`:620`) остаётся |
| `test_lexer_performance_large_file` | `integration_tests.rs:635`, `:653` | производительность, функциональных утверждений нет | O |
| `test_format_real_code_performance` | `crates/ide/src/formatting/engine.rs:331`, `:385` | производительность, только печать и время | O |
| `test_format_large_file_performance` | `crates/ide/src/formatting/engine.rs:389`, `:429-430` | то же | O |

Результат шага: в обычном профиле нет сравнения времени с границей < 10 с.
Каждое такое утверждение выполняется под `--cfg timing_contract`.

### Шаг 6. Наблюдавшиеся флейки с механизмом в тесте (SEEN)

Падения добыты командой A8: 100 последних упавших run по базе. Ни одно не
воспроизводилось локально. Механизм — ФАКТ по коду; что сработал именно он, —
ГИПОТЕЗА, кроме g1 (шаг 2).

| Тест | Run и симптом | Механизм в коде | Решение |
|---|---|---|---|
| `tools::graph::tests::graph_data_requests_are_pool_only_under_held_lease` (`crates/mcp-server/src/tools/graph.rs:547`) | 3 падения на Windows, `:564` `expect("a resident descriptor needs no lease refresh")` | `wait_ready` (`:558`) не ждёт проход публикации, который проверяет владение. Тест держит лок и сбросил вердикт (`:559-561`). Любая проверка в окне — проход или heartbeat — даёт `Paused` (цепочка — п. 4 «Устройства»), и `snapshot()` возвращает `None` (`snapshot.rs:380-384`) | Н: `wait_settled`; затем `hold_lifecycle_lock_for_test()` на всё окно — heartbeat и любая проверка встают на мьютексе `checked_at` (`workspace_lease.rs:601`). Дескрипторы берутся в потоке под потолком: регрессия «snapshot ходит в lease» повиснет и упадёт по сторожу. «Не ходил за lease» проверяет журнал дисковых проверок, как `snapshot.rs:2359-2366`. `< 100 мс` (`:569`) — Т. CI `:56`, `:584` |
| `graph::state::tests::a_chronically_unreadable_subtree_is_probed_but_never_rebuilt` (`state.rs:9434`) | runs 38606466398, 38776883909: `the probe did not back off: 60s then 60s` (`:9468-9471`) | интервал удваивается только при `NoNewEvidence` (`crates/mcp-server/src/graph/debt.rs:1753`); занятая проба фонового обхода (`debt.rs:1615-1619`, `watcher.rs:1557`) оставляет 60 с; тест ждёт `wait_ready` и `!in_flight` (`:9451-9452`), но не `!walk_out()` | Н: `wait_settled`, перед каждым `probe_now` — `wait_until(!walk_out())`. CI `:190` |
| `graph::state::tests::a_healed_subtree_is_found_by_the_probe_and_rebuilt` (`state.rs:6522`) | run 38300123498: тайм-аут `wait_until` (`:6559`) | один `probe_now` и `drive` (`:6556-6557`) при возможном занятом резерве; `wait_ready` (`:6540`) | Н: `wait_settled`; `!walk_out()` до `probe_now`. Повторных `drive` в цикле нет. CI `:188` |
| `graph::state::tests::a_build_that_never_started_spends_no_healing` (`state.rs:8480`) | runs 38606466398, 38640930395: `…: Some(Running)` (`:8552`) | отказ спавна сразу пишет `Failed` (`state.rs:2979-2984`); `Running` значит, что reload другого драйвера ещё идёт; тест не ждёт его перед `loader_cannot_spawn` (`:8539`) | Н: `wait_until(reload != Running)` перед `:8539`. CI `:281`, `:789` |
| `graph::build::tests::a_failed_reload_whose_retry_is_held_stays_owed` (`crates/mcp-server/src/graph/build.rs:4423`) | run 38322192087: `build.rs:4445` | `wake_at(now)` видит долг, только пока `held_until > now`; `HELD_RETRY` = 2 с (`debt.rs:40`, `:1562-1563`, `:2140-2141`) | Н: проверить долг геттером `owes_failed()` (как `state.rs:8555`); исходная проверка через `wake_at(now)` — Т. CI `:116`, `:644` |
| фикстура `PayloadServer` (`crates/bsl-search/src/embedder/payload_tests.rs:28-30`), упал `payload_overlay_prime_installs_the_lexical_overlay_before_reporting_embedding_failure` (`crates/bsl-search/src/engine/lifecycle_tests.rs:589`) | run 38621322100, Windows: `payload fixture exceeded its watchdog` | дедлайн 30 с отсчитывается от создания фикстуры и проверяется на каждом проходе accept, так что он меряет длительность всего теста. От зависания не защищает: выход — по `stop` из `Drop` (`:98-106`) | Н: убрать абсолютный дедлайн; выход по `stop`, чтение с тайм-аутом `:41`. CI `:308`, `:387` (фильтр `lifecycle`) |
| `workspace_cache_scope_explicit_workspace_build_namespace_preserves_flat_canaries` (`crates/bsl-analyzer/tests/source_set_cli.rs:1894`) | runs 38628192504, 38790778658 (develop `4485c3d4`): `source_set_cli.rs:380:56` `NotFound` | `walk` (`:379-421`) делает `symlink_metadata(...).unwrap()` записи, которую вернул `read_dir`, пока сервер жив (`:1918`); heartbeat пишет `writer.tmp.<pid>` и переименовывает его (`workspace_lease.rs:1273-1275`) | Н: в `walk` `NotFound` у записи означает «записи нет в снимке». Исчезновение источника по-прежнему ловит `assert_tree_unchanged_except_workspace_cache` (`:455`). CI `:36`, `:367` |

Уже исправлено в базе, не меняется:
- `vector_lifecycle_fused…` — `5b65205d`;
- `a_refused_install_retires_nothing` — `2c5dbc62`;
- `concurrent_probe_callers_share_one_reservation` — `90f85a02`;
- `diagnostics_selective_baseline_config_reload_changes_result_id` — `ef655fc4`;
- `lsp_serves_the_configured_corpus_until_restart` — `e9c24620`.

### Шаг 7. Тесты, которые нельзя сделать независимыми без продуктового хука (O)

Пауза замены ограничена константой `INSTALL_READERS_WAIT` = 5 с
(`build.rs:1600`, `:2141`). Тест должен внутри неё синхронизироваться с паузой
(одно `Busy` стоит `ADMISSION_WAIT` = 2 с) и отпустить чтение: запас 3–4,7 с,
без хука его не расширить. Регрессия здесь не превращается в зависание.

| Тест | Место | Решение |
|---|---|---|
| `a_replacement_waits_for_a_held_read_then_serves_the_next_generation` | `crates/mcp-server/src/graph/build.rs:2692`, `:2711-2716` | O; при запуске `sleep(300)` заменяется синхронизацией «читать с `ZERO`, пока не придёт `Busy`» (`snapshot.rs:380`) |
| `a_replacement_waits_for_four_concurrent_reads` | `crates/mcp-server/src/graph/build.rs:2752`, `:2775-2781` | O; та же синхронизация вместо `sleep(150)` |
| `a_read_during_the_pause_is_refused_after_the_admission_wait` | `crates/mcp-server/src/graph/build.rs:2790`, `:2808-2814` | O; та же синхронизация; окно `[1900, 3500)` сохраняется |

В обязательном прогоне остаётся «читатель, переживший ожидание, сохраняет
старый граф и замену»: `a_read_outlasting_the_wait_keeps_the_old_graph_and_the_replacement`
(`build.rs:2822`), временное утверждение — только нижняя граница `:2837`.
Верхнюю границу ожидания читателей проверяют лишь тесты шага в `timing_contract`.

### Шаг 8. Сверка

- Повторный запуск A1–A3, A10, A11 и команд таблицы проверок.
- Мутационные проверки (раздел «Таблица проверок») выполняются локально и в
  PR не коммитятся.

## Инварианты

- **И1. Opt-in профиль.**
  - Вход: `cargo test --workspace` без cfg и с `RUSTFLAGS="--cfg timing_contract"`.
  - Свойство: тесты O (шаги 5 и 7) в первом случае `ignored`, во втором
    выполняются и проходят.
  - Каждое утверждение Т выполняется только во втором.
- **И2. Порядок.**
  - Вход: в канал клиента перед ответом на запрос 731 (и на запросы двух
    соседних тестов) положено уведомление `textDocument/publishDiagnostics`.
  - Свойство: три теста шага 2 проходят.
- **И3. Сводки lifecycle.**
  - Вход: операция из N мутаций с любым числом срабатываний таймера, в том
    числе с паузой ≥ 1,1 с внутри интервала.
  - Свойства: `started = 1 + pending + progress`; `files ≤ 128` у каждой
    записи; Σ`files` = N; `started` без интервальных данных.
- **И4. Нет коротких положительных сроков.**
  - Вход: тестовый код после PR, сканеры A2, A10 и A11.
  - Свойство: каждая строка A2 и A10 есть в Приложении Б с классом NEG, SYNC,
    PACE или «не ожидание». Каждая константа A11 есть там же, и ни одна не
    задаёт срок положительного ожидания вне `timing_contract`. Номера строк
    могут сдвинуться.
  - Срок каждого положительного ожидания из строки с решением В — константа
    потолка допущения 2. Литеральные сроки остаются только у строк
    Приложения Б.
- **И5. Нет временных верхних границ < 10 с в обычном профиле.**
  - Вход: сканер A1f после PR.
  - Сканер расширяется: печатает, стоит ли строка под `timing_contract`.
  - Свойство: каждая строка вне `timing_contract` — одно из трёх:
    - нижняя граница;
    - граница ≥ 10 с (WATCH, Б/2, В);
    - строка A1f-части Приложения Б, где сравнивается не время.
- **И6. Нет sleep-синхронизации перед положительным утверждением.**
  - Вход: тесты с решением Н в шагах 4 и 6.
  - Свойство: если убрать из теста любой оставшийся `sleep`, кроме
    помеченных в строке теста как нижняя граница, вердикт на корректном коде
    не меняется.
- **И7. Граница Б/2.**
  - Вход: девять тестов с решением Б/2 в шаге 5.
  - Свойство: граница ≥ 10 с и < B.
  - На продукте, который ждёт свой бюджет (локальная мутация), тест падает.
- **И8. Пул под удержанной lease.**
  - Вход: `graph_data_requests_are_pool_only_under_held_lease` плюс поток,
    который всё окно в цикле зовёт `lease.owns_caches()` (имитация heartbeat).
  - Свойство: тест проходит.
- **И9. Ожидания графа в SEEN.** У каждого теста шага 6 свой вход, по его
  механизму:
  - `a_chronically_unreadable_subtree_is_probed_but_never_rebuilt` и
    `a_healed_subtree_is_found_by_the_probe_and_rebuilt` — перед окном
    проверки параллельно зарезервирована проба (имитация фонового обхода);
  - `a_build_that_never_started_spends_no_healing` — reload до `:8539`
    удерживается в `Running` хуком сборки
    (`with_build_candidate_hook_for_test`, `state.rs:1676`; хук вызывается в
    `build_candidate`, `build.rs:1767`, на пути `spawn_reload`, `state.rs:2963`). Поток снимает
    удержание, когда замыкание ожидания `reload != Running` выполнилось хотя
    бы дважды (счётчик в замыкании);
  - `a_failed_reload_whose_retry_is_held_stays_owed` — между `drive()`
    (`build.rs:4442`) и `wake_at(now)` (`:4445`) пауза `HELD_RETRY` + 100 мс.
    Это нижняя граница, поэтому срок удержания истекает гарантированно.

  Свойство:
  - на базе каждый тест падает с тем же текстом, что в CI;
  - после PR все четыре проходят на том же входе. Реализация, которая
    не ждёт, а сразу проверяет `reload != Running`, на входе третьего теста
    падает.
- **И10. Фикстура payload.**
  - Вход: фикстура `PayloadServer`, которая живёт дольше 30 с (локально —
    дедлайн 0 с на базе).
  - Свойство: на базе тесты фикстуры падают `exceeded its watchdog`, после PR
    проходят.
- **И11. Снимок дерева.**
  - Вход: `walk` над каталогом, запись которого исчезла между `read_dir` и
    `symlink_metadata`.
  - Свойство: снимок не паникует. Исчезновение отслеживаемого источника
    по-прежнему даёт `source path disappeared`.
- **И12. Нет потерь.**
  - Вход: `cargo test --workspace -- --list` до и после.
  - Свойство: список тестов после ⊇ список до.
  - Ни одна проверка шага 5 с пометкой «остаётся» не удалена.
- **И13. Фильтры CI.**
  - Вход: `ci.yml` и `scripts/test-ci-test-output.py`.
  - Свойство: `ci.yml` не меняется; каждый `run_exact`/`run_nonempty`
    выполняет ≥ 1 тест; скрипт проходит.
- **И14. Только тесты.**
  - Вход: `git diff 5b65205d...HEAD`.
  - Свойство: каждая изменённая строка лежит в тестовом коде: в файле под
    `tests/` или `*_tests.rs`, в модуле, который родитель объявляет под
    `#[cfg(test)]`, или внутри блока `#[cfg(test)] mod … { }` (по скобкам).
    Атрибут на `use` вне такого блока (`graph/build.rs:6`) не в счёт.
  - Вне `crates/` меняются только `Cargo.toml:20` и
    `docs/contributing/DEVELOPMENT_RULES.md`.

## Таблица проверок

| Обещание или инвариант | Проверка | Положительный контроль | Критерий |
|---|---|---|---|
| И1 | `cargo test -p parser --test integration_tests -- --list --ignored`, `cargo test -p ide formatting::engine -- --list --ignored` и `cargo test -p mcp-server --lib graph::build::tests -- --list --ignored`, без cfg и с `RUSTFLAGS="--cfg timing_contract"` | без cfg список содержит три теста O шага 5 и три теста шага 7 | с cfg их нет среди ignored; `CARGO_TARGET_DIR=target-timing RUSTFLAGS="--cfg timing_contract" cargo test --workspace` выполняет тесты O и утверждения Т, и они проходят |
| И2 | три теста шага 2 | локальная мутация: перед последним `handle_task` отправить в канал клиента `Notification` — на базе `:570` падает `expected retry response` | после PR тесты проходят с той же мутацией и без неё |
| И3 | тесты шага 3 и новый тест с паузой 1,1 с | на базе с той же паузой точное `records.len()` ложно | новый тест и тесты шага 3 проходят |
| И4 | сканеры A2, A10 и A11 после PR; плюс команда A12 по телам тестов с решением В — каждое найденное место либо строка Приложения Б, либо не срок ожидания | на базе 44 из 82 строк A2 (82 − 25 − 13) и 15 из 28 строк A10 без допустимого класса; `SHUTDOWN_TIMEOUT` и `HANG_GUARD` задают положительные сроки | всё — как в И4 |
| И5 | расширенный A1f после PR | на базе A1f = 93 строки, из них под `timing_contract` 0, и верхних границ < 10 с среди них больше нуля (например `build.rs:4041`) | вне `timing_contract` — только нижние границы, границы ≥ 10 с и строки Приложения Б без времени |
| И6 | для каждого теста Н: локально удалить оставшиеся `sleep`, кроме помеченных как нижняя граница, и прогнать тест | на базе удаление `sleep(HOLD)` в `try_acquire_engine_queues_until_the_lock_frees` (`acquire.rs:534`) роняет `waited ≥ HOLD/2`: удаление `sleep` видно | после PR каждый тест Н проходит без `sleep` |
| И7 | для каждого теста Б/2 — локальная мутация продукта, при которой код ждёт свой бюджет (например, sweeper не будится при shutdown) | мутация — сам контроль: без неё тест проходит, с ней обязан упасть | с мутацией тест падает на границе `B/2`; без мутации проходит |
| И8 | `graph_data_requests_are_pool_only_under_held_lease` с потоком-имитатором heartbeat | на базе с тем же потоком `:564` падает `a resident descriptor needs no lease refresh` | после PR проходит |
| И9 | четыре теста `graph` шага 6, каждый со своим входом из И9 | на базе — тот же текст паники, что в CI | после PR — как сказано в И9 |
| И10 | тесты фикстуры `PayloadServer` | на базе с дедлайном 0 с — `payload fixture exceeded its watchdog` | после PR проходят; дедлайна в фикстуре нет |
| И11 | новый тест помощника `walk` с исчезающей записью и тест с исчезающим источником | на базе первый падает `NotFound` на `:380` | после PR первый проходит, второй падает `source path disappeared` |
| И12 | команда A13 до и после | число до | после ≥ до; `git diff` не удаляет ни одного `assert` шага 5 с пометкой «остаётся» |
| И13 | `python3 scripts/test-ci-test-output.py`; `git diff --stat -- .github/workflows/ci.yml`; CI PR | локально `run_nonempty` с фильтром, который ничего не выбирает (`… --lib no_such_test_name`), возвращает ненулевой код | скрипт проходит; дифф `ci.yml` пуст; шаги CI зелёные |
| И14 | скрипт по правилу И14 над `git diff -U0 5b65205d...HEAD` | на базе с искусственной правкой непроверочной функции скрипт её печатает | вывод пуст |
| Шаг 1: `clippy` не ловит `unexpected_cfgs` | `cargo clippy --all-targets --all-features -- -D warnings` | без строки `check-cfg` — предупреждение `unexpected_cfgs` | проходит |

## Открытые вопросы

1. **Паника сервера в s6** (run 38800079084): причина неизвестна, stderr
   отброшен (`crates/bsl-analyzer/tests/common/mod.rs:100`). Захват stderr в
   харнессе — отдельная задача.
2. **Хуки в непроверочном коде.** `#[cfg(test)]`-хук на
   `INSTALL_READERS_WAIT` или доступ к `installing` вернул бы в обязательный
   прогон три теста шага 7. Это решение владельца: формально это правка
   production-файла, поэтому в PR её нет.
3. **Класс `wait_ready` → состояние прохода** (61 место, A4), без наблюдавшихся
   падений. Нужен поштучный разбор отдельной задачей.
4. **Возможные дефекты продукта.** Сюда входят:
   - `a_revision_raised_during_an_offer_survives_that_offers_reply`;
   - `a_decision_that_claims_no_build_spends_no_mark_attempt`;
   - `workspace_cache_default_base_inside_nested_workspace_is_rejected_before_creation`;
   - s5 `TransportClosed` — наблюдатель конфигурации может оборвать MCP-сессии
     на частично записанном файле (`crates/mcp-server/src/graph/watcher.rs:52-70`).

   Нужны трассы. Правка тестов их бы скрыла.
5. **Попутная находка вне класса нагрузки.** `store.rs:7448-7451` фильтрует
   записи `kind == "mutation"`, а при уровне INFO их нет: мутация с родителем
   пишется на уровне DEBUG (`emit(parent.is_some())`,
   `crates/bsl-search/src/lifecycle.rs:321-327`, `:559`, `:573`). Проверка
   проходит вхолостую.

## Приложение А. Команды множеств

Сканеры лежат вне репозитория и печатают `файл:строка:функция: текст`. Тестовый
код — файлы под `tests/`, `*_tests.rs`, `tests.rs` и строки после первого
`#[cfg(test)]` файла.

- **A1** (`scan.py`): строки `assert!`, где есть слово времени (`elapsed`,
  `Instant`, `waited`, `Duration::from…`) и операция сравнения.
  `python3 scan.py . | wc -l` → 156.
- **A1f**: `python3 scan.py . | grep -vE 'now\(\) < \w*deadline' | wc -l` → 93.
  Одна строка из 93 — `elapsed_ns > 0`
  (`crates/bsl-analyzer/src/bench/unread_module_tests.rs:102`, не время).
- **A2** (`scan_short.py`): вызовы
  `recv_timeout|wait_timeout|wait_empty|wait_ready|watch_readiness|wait_for_within|timeout|wait_until*|join_timeout`
  с литералом `Duration::from_{millis,secs}` < 10 000 мс → 82.
- **A3** (`scan_sleep.py`): `thread::sleep(`/`time::sleep(` → 421;
  `… | grep -c 'diagnostics_state/drift.rs'` → 87.
- **A4**: `wait_ready(&` в `crates/mcp-server/src/{graph/*,lib.rs,tools/**}`, за
  которым в двух непустых строках есть `.snapshot()`, `debt`, `probe`, `hook`,
  `in_flight`, `builders_started`, `owes_`, `walk_out`, `full_builds_started`
  → 61.
- **A5**: `rg -c 'cfg_attr\(not\(corpus_contract\)' crates | awk -F: '{s+=$2} END {print s}'` → 512.
- **A6**: для каждого `crates/*/Cargo.toml` — наличие `[lints] workspace = true`.
  Не наследует только `crates/check-contract`.
- **A7**: `grep -nE 'test_small_file_performance|test_lexer_performance|test_format_real_code|test_format_large_file|insert_pair_hot_target|rendering_cost|a_replacement_waits_for|a_read_during_the_pause' .github/workflows/ci.yml` → 0 строк.
- **A8**: `gh run list --status failure --limit 100`, затем
  `gh run view <id> --log-failed | grep -E '\.\.\. FAILED|panicked at|test result: FAILED'`
  по каждому run.
- **A10** (`scan_deadline.py`): `(Instant::now()|now) + Duration::from_{millis,secs}(N)`
  с N < 10 000 мс в тестовом коде → 28.
- **A11** (`scan_const.py`): `const X: Duration = Duration::from_…(N)` с
  N < 10 000 мс в тестовом коде → 28, у каждой — места использования.
- **A12**: `rg -n 'Duration::from_(secs|millis)\('` по телам тестов с
  решением В.
- **A13**: `cargo test --workspace -- --list 2>/dev/null | grep -c ': test$'`.
- **A9**: строки A2 и A1f, у которых имя функции (третье поле вывода) не
  встречается в обратных кавычках в шагах 2–7 плана → 29 и 29.

## Приложение Б. Строки сканеров, которые остаются по классу

Это строки A2 и A1f, чья функция не названа в шагах 2–7 (вывод команды A9:
29 из 82 и 29 из 93), плюс 13 строк A2 внутри тестов шагов, которые шаги не
меняют. Пути относительно `crates/`.

**A2: 29 строк вне тестов шагов**

| Строки | Класс | Почему вердикт не зависит от скорости |
|---|---|---|
| `bsl-analyzer/src/call_hierarchy_index_state_tests.rs:62`, `:195`, `:255`, `:276` | SYNC | `notify` синхронен, событие уже в канале |
| `bsl-analyzer/src/server.rs:2329` | SYNC | очередь заполнена до чтения |
| `bsl-analyzer/tests/diagnostics_baseline_lsp.rs:64`; `selective_diagnostics_baseline_lsp.rs:165`, `:299` | NEG | окно отсутствия сообщения |
| `bsl-analyzer/tests/diagnostics_baseline_lsp.rs:125`, `:156` | NEG | `wait_for_within(250 мс)` ждёт, что сообщения не будет |
| `bsl-analyzer/tests/lsp_semantic_tokens_refresh.rs:63`, `:120`; `partitioned_diagnostics_baseline_lsp.rs:62`, `:177`; `selective_diagnostics_baseline_lsp.rs:141`, `:186`, `:206`, `:255` | PACE | окно опроса внутри цикла с дедлайном ≥ 10 с |
| `bsl-analyzer/tests/platform_help_process.rs:161` | PACE | только текст на пути ошибки |
| `bsl-search/src/engine.rs:12538` | PACE | `busy_timeout(0)` у держателя, ожидания нет |
| `bsl-search/src/publish.rs:1010`, `:1037` | в шаге 4 | помощник `embed_batch` теста `payload_shared_publish_preserves_worker_concurrency_bound` |
| `bsl-search/src/store.rs:7819`, `:7892` | в шаге 4 | помощник `release_after_peer_commit` теста `context_observer_matches_original_sql_with_competing_writer_and_rollback` |
| `mcp-server/src/baseline.rs:2207` | SYNC | слот уже готов до ожидания |
| `mcp-server/src/change_hub.rs:10660`; `workspace_lease.rs:2488`; `state/sync.rs:2061`; `mcp-server/tests/broker.rs:401` | NEG | ожидается `NotYet`, `is_err()` или тайм-аут |

**A2: 13 строк внутри тестов шагов, которые не меняются**

| Строки | Класс | Почему вердикт не зависит от скорости |
|---|---|---|
| `bsl-analyzer/src/server.rs:3660`, `:3692` | SYNC | пакет отправлен тем же потоком до чтения (`:3654`, `:3667`) |
| `bsl-analyzer/src/handlers/dispatch.rs:1126` | SYNC | `handle_task` выполнен на том же потоке (`:1123`) |
| `bsl-analyzer/tests/platform_help_process.rs:229` | PACE | окно внутри цикла с повторной провокацией: медленность ведёт к следующему проходу, а не к падению |
| `bsl-search/src/engine.rs:12588` | PACE | `busy_timeout(0)`, ожидания нет |
| `mcp-server/src/baseline.rs:2146`; `change_hub.rs:6667`, `:10688`; `state/mod.rs:1270`, `:1328` | NEG | ожидается отсутствие события |
| `vfs-notify/src/lib.rs:1821`, `:1825`, `:1896` | PACE | шаг опроса внутри цикла, чей дедлайн становится В |

**A1f, 29 строк**

| Строки | Класс | Почему вердикт не зависит от скорости |
|---|---|---|
| `bsl-analyzer/src/bench/unread_module_tests.rs:102` | — | `elapsed_ns > 0`, не граница |
| `bsl-analyzer/src/handlers/request.rs:3426` | WATCH | `wait_bound` = `PARK_HANG_GUARD` 30 с |
| `bsl-analyzer/tests/common/mod.rs:65`, `:222`; `selective_diagnostics_baseline_lsp.rs:189`, `:211`; `mcp-server/src/diagnostics_state/lifecycle.rs:458`; `mcp-server/src/indexing_runtime_tests.rs:339`; `mcp-server/src/state/sync.rs:3144`; `bsl-search/src/engine.rs:8602` | WATCH | дедлайны 10 с – 10 мин |
| `bsl-analyzer/tests/shared_workspace_analysis_s5.rs:322` | — | проверка состояния, не времени |
| `mcp-server/src/baseline.rs:2993`; `change_hub.rs:10315`; `diagnostics_state/session.rs:731`; `state/sync.rs:3140`; `tools/search/acquire.rs:559` | LOWER | нижняя граница |
| `mcp-server/src/graph/build.rs:4413` | LOWER | `due > now + 10 с`, где `due` отсчитан от отказа: медленность сокращает запас «бэкофф − 10 с». При реализации проверить, что запас ≥ 10 с; иначе — Т |
| `mcp-server/src/baseline.rs:2995`, `:3014` | — | дизъюнкция состояний, не времени |
| `mcp-server/src/change_hub.rs:10287` | SYNC | уведомление синхронно поднимает generation |
| `mcp-server/src/graph/state.rs:9983` | — | виртуальное время |
| `mcp-server/src/state/embed.rs:5075`; `state/overlay_retry.rs:1577` | — | сравнение констант |
| `mcp-server/src/state/overlay_backlog.rs:1902`, `:1929` | LOWER | остаток расписания считается от текущих часов, медленность его только уменьшает |
| `mcp-server/tests/references.rs:1959`, `:2001` | — | размер вывода, не время |
| `ide/tests/partitioned_baseline_scale.rs:206` | вне PR | `#[ignore]`, только push-only job (`ci.yml:798`), см. «Вне рамок» п. 6 |
| `parser/tests/integration_tests.rs:858` | — | время только в тексте сообщения, утверждение — `result.is_ok()` |

**A10: 13 из 28 строк, которые остаются**

Остальные 15 строк A10 лежат в тестах и помощниках шагов 4 и 6 с решением В.

| Строки | Класс | Почему вердикт не зависит от скорости |
|---|---|---|
| `bsl-analyzer/tests/diagnostics_baseline_lsp.rs:62`; `selective_diagnostics_baseline_lsp.rs:163`, `:297` | NEG | срок окна отсутствия сообщения |
| `bsl-analyzer/tests/source_set_cli.rs:832`, `:840` | не ожидание | уборка процесса `stop_owned_daemon_after_timeout`: утверждения нет |
| `mcp-server/src/tasks.rs:530` | не ожидание | срок — вход продукта: тест проверяет, что срок сработал, и медленность только откладывает это |
| `mcp-server/src/graph/debt.rs:2986`, `:3285`, `:4401`, `:4717`; `state/overlay_backlog.rs:1227` | не ожидание | арифметика виртуального времени |
| `mcp-server/src/state/overlay_backlog.rs:1591` | NEG | окно, в котором лок не должен браться |
| `mcp-server/src/graph/watcher.rs:843` | не ожидание | срок `due`, который тест задаёт продукту (`a_failed_graph_is_retried_when_its_backoff_runs_out`, шаг 4) |

**A11: 28 констант**

| Строки | Класс |
|---|---|
| `mcp-server/src/`: `graph_db.rs:1994`, `:1995`; `lib.rs:1017`; `graph/build.rs:1592`, `:1600`; `graph/snapshot.rs:59`, `:299`; `graph/state.rs:26`, `:3097`; `tools/search/acquire.rs:302`; `baseline.rs:342`; `workspace_lease.rs:74`, `:79`, `:83` | не тест: продуктовая константа ниже первого `#[cfg(test)]` файла |
| `bsl-analyzer/tests/common/mod.rs:25`; `mcp-server/src/state/mod.rs:1763`, `:1804`; `mcp-server/tests/task_deadline.rs:35`, `tests/tasks.rs:51`, `tests/common/mod.rs:46` | PACE: шаг опроса |
| `bsl-analyzer/tests/common/mod.rs:26` | не ожидание: уборка после сбоя |
| `mcp-server/src/lib.rs:3618`, `:5945`; `serve_stream_tests.rs:16`; `tools/search/acquire.rs:517`; `cancel_tests.rs:35`, `:122`; `mcp-server/tests/tasks.rs:43` | в шагах 4–5 |
