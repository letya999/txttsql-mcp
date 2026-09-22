# Быстрый старт

Нужны Rust 1.96+, Cargo и доступ к crates.io для первой сборки.

```sh
cp config.example.toml config.toml
# Заполните адреса и ссылки на секреты в config.toml.
cargo test --locked
cargo run --locked -- --config config.toml
```

Для stdio MCP stdout зарезервирован под протокол. Логи и ошибки запуска идут в stderr. В клиенте MCP укажите команду `txttsql-mcp` с аргументами `--config` и абсолютным путём к конфигу.

`allowed_schemas` и `allowed_tables` управляют доступом отдельно для каждого источника. Физические таблицы в SQL и в `allowed_tables` указывайте как `schema.table` без кавычек. Пустые списки не дают доступа к таблицам. `SELECT 1` допустим без таблиц. Учётную запись базы всё равно нужно ограничить чтением на стороне сервера.

В MCP сначала вызовите `list_sources`, затем `list_tables` и `describe_table` для нужного источника. Уточните определения метрик через `find_context`/`explore_graph` или подключённый каталог, затем проверьте SQL через `validate_sql` и выполните `execute_sql`.
