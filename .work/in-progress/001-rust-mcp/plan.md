# Реализация Rust MCP

1. Инвентаризовать `txttsql`, шаблон memory bank и первичные источники MCP.
2. Реализовать источник, секреты, SQL guard, DB adapter registry и stdio MCP.
3. Добавить опциональную память, онтологию, OpenMetadata и Airflow.
4. Проверить Cargo gates, протокол MCP, Docker и безопасность; исправить найденное.

Проверено 22–23.09.2026: `just ci`, HTTP-тесты OpenMetadata/Airflow v1/v2 с Bearer/Basic, независимые live-тесты PostgreSQL/CockroachDB/ClickHouse, одновременные MCP-вызовы всех трёх БД с каталогом таблиц и столбцов, чтение реального графа `txttsql/memory`, security scan/Opengrep и Docker build + stdio smoke (13 инструментов). После коммита `4d813ee` дополнительно проверен Airflow 2.10.5 в одноразовом контейнере: поиск DAG, задачи и связи. Проверен TLS с собственным CA: локальный HTTPS-сервер отклоняет подключение без PEM и принимает с ним, одноразовый PostgreSQL с TLS проходит `pgwire_live`. Интеграция OpenMetadata проверена локальным HTTP-сервером; публичный sandbox отвечает 401, поэтому live-проверка на самом сервисе возможна только с тестовым токеном.

Повторный аудит 23.09.2026: по [ресурсу таблиц OpenMetadata](https://github.com/open-metadata/OpenMetadata/blob/main/openmetadata-service/src/main/java/org/openmetadata/service/resources/databases/TableResource.java) `description` не входит в параметр `fields`; запрос исправлен на `columns,tags,owners`, контрактный тест проверяет параметр. PostgreSQL/CockroachDB теперь читает строки потоково с общим пределом ответа 16 МБ; превышение проверено на одноразовом PostgreSQL. Добавлен тест прямого запуска credential broker. Реальный OpenMetadata отсутствует по сообщению владельца; его live-тест остаётся доступен через `OPENMETADATA_TEST_*`.

Проверка Docker с включённой памятью обнаружила отсутствие прав записи в свежем томе `/data` при UID `65532`. Исправлен владелец каталога в образе; проверка реального вызова `search_saved_queries` через MCP и новый Docker volume проходит локально и добавлена в CI.

Сквозная проверка Antigravity CLI 23.09.2026: установлен stdio MCP `txttsql-local-e2e`, через агента вызваны все 13 инструментов на тестовых PostgreSQL, CockroachDB, ClickHouse, SQLite-плагине, metadata-плагине, онтологии и памяти. Локальные HTTP-макеты подтвердили Bearer/Basic для встроенных OpenMetadata/Airflow и OpenMetadata-плагина; проверены SQL guard, allowlist и лимит строк. Вызов HTTP credential broker из Antigravity выявил наследование открытого stdin MCP-клиента дочерним процессом. Broker теперь получает закрытый stdin; регрессионный тест воспроизводит прежний сбой без исправления и проходит с ним. Реальный round-trip Windows Credential Manager остаётся непроверенным: ОС возвращает ошибку 8 при создании одноразовой записи.
