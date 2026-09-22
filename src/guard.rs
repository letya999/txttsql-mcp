use crate::config::{DatabaseKind, Source};
use anyhow::{Result, bail};
use sqlparser::{
    ast::{Expr, ObjectName, Query, SetExpr, Statement, TableFactor, Visit, Visitor},
    dialect::{ClickHouseDialect, PostgreSqlDialect},
    parser::Parser,
};
use std::{collections::HashSet, ops::ControlFlow};

#[derive(Debug, Clone)]
pub struct ApprovedQuery {
    pub sql: String,
    pub tables: Vec<String>,
    pub row_cap: u32,
}

pub fn table_allowed(source: &Source, name: &str) -> bool {
    let parts: Vec<_> = name.split('.').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            part.bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        && (source
            .allowed_tables
            .iter()
            .any(|table| same_name(source.kind, table, name))
            || source
                .allowed_schemas
                .iter()
                .any(|schema| same_name(source.kind, schema, parts[0])))
}

fn same_name(kind: DatabaseKind, left: &str, right: &str) -> bool {
    match kind {
        DatabaseKind::Clickhouse => left == right,
        _ => left.eq_ignore_ascii_case(right),
    }
}

fn normalize_name(kind: DatabaseKind, name: &str) -> String {
    match kind {
        DatabaseKind::Clickhouse => name.to_owned(),
        _ => name.to_ascii_lowercase(),
    }
}

pub fn validate(source: &Source, sql: &str, requested_rows: Option<u32>) -> Result<ApprovedQuery> {
    if sql.len() > 100_000 {
        bail!("SQL exceeds 100 KB")
    }
    let dialect: Box<dyn sqlparser::dialect::Dialect> = match source.kind {
        DatabaseKind::Clickhouse => Box::new(ClickHouseDialect {}),
        _ => Box::new(PostgreSqlDialect {}),
    };
    let mut statements = Parser::parse_sql(dialect.as_ref(), sql)
        .map_err(|_| anyhow::anyhow!("SQL did not parse for this dialect"))?;
    if statements.len() != 1 {
        bail!("exactly one statement is required")
    }
    let statement = statements.remove(0);
    if !matches!(statement, Statement::Query(_)) {
        bail!("only SELECT queries are allowed")
    }
    let mut visitor = GuardVisitor {
        source,
        scopes: Vec::new(),
        tables: HashSet::new(),
    };
    if let ControlFlow::Break(reason) = statement.visit(&mut visitor) {
        bail!("{reason}")
    }
    let row_cap = requested_rows
        .unwrap_or(source.max_rows)
        .min(source.max_rows);
    if row_cap == 0 {
        bail!("row limit must be positive")
    }
    // AST serialization removes trailing comments and semicolons; the outer SELECT enforces
    // a hard row cap even if the input already contains an arbitrarily large LIMIT.
    let wrapped = format!(
        "SELECT * FROM ({statement}) AS _txttsql_rows LIMIT {}",
        row_cap + 1
    );
    let mut tables: Vec<_> = visitor.tables.into_iter().collect();
    tables.sort();
    Ok(ApprovedQuery {
        sql: wrapped,
        tables,
        row_cap,
    })
}

struct GuardVisitor<'a> {
    source: &'a Source,
    scopes: Vec<HashSet<String>>,
    tables: HashSet<String>,
}

impl Visitor for GuardVisitor<'_> {
    type Break = String;

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Self::Break> {
        if !matches!(statement, Statement::Query(_)) {
            return ControlFlow::Break("only SELECT queries are allowed".into());
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Self::Break> {
        if !query.locks.is_empty()
            || query.settings.is_some()
            || query.format_clause.is_some()
            || query.for_clause.is_some()
        {
            return ControlFlow::Break(
                "locking, SETTINGS, FORMAT and output clauses are forbidden".into(),
            );
        }
        if let Err(reason) = check_set_expr(&query.body) {
            return ControlFlow::Break(reason.into());
        }
        let aliases = query
            .with
            .as_ref()
            .map(|with| {
                with.cte_tables
                    .iter()
                    .map(|cte| normalize_name(self.source.kind, &cte.alias.name.value))
                    .collect()
            })
            .unwrap_or_default();
        self.scopes.push(aliases);
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _: &Query) -> ControlFlow<Self::Break> {
        self.scopes.pop();
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, table: &TableFactor) -> ControlFlow<Self::Break> {
        match table {
            TableFactor::Table { args: None, .. }
            | TableFactor::Derived { .. }
            | TableFactor::NestedJoin { .. } => ControlFlow::Continue(()),
            _ => ControlFlow::Break(
                "table functions and unsupported FROM forms are forbidden".into(),
            ),
        }
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<Self::Break> {
        let parts: Vec<_> = relation
            .0
            .iter()
            .filter_map(|part| part.as_ident())
            .collect();
        if parts.len() != relation.0.len()
            || parts.iter().any(|part| {
                part.quote_style.is_some()
                    || part.value.is_empty()
                    || !part
                        .value
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        {
            return ControlFlow::Break("quoted or unsupported table names are forbidden".into());
        }
        let name = parts
            .iter()
            .map(|part| normalize_name(self.source.kind, &part.value))
            .collect::<Vec<_>>()
            .join(".");
        if parts.len() == 1 && self.scopes.iter().rev().any(|scope| scope.contains(&name)) {
            return ControlFlow::Continue(());
        }
        if parts.len() != 2 {
            return ControlFlow::Break("physical tables must use schema.table names".into());
        }
        if !table_allowed(self.source, &name) {
            return ControlFlow::Break(format!("table {name} is outside the source allowlist"));
        }
        self.tables.insert(name);
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Self::Break> {
        if let Expr::Function(function) = expr {
            let parts: Vec<_> = function
                .name
                .0
                .iter()
                .filter_map(|part| part.as_ident())
                .collect();
            if parts.len() != function.name.0.len()
                || parts.iter().any(|part| part.quote_style.is_some())
            {
                return ControlFlow::Break(
                    "quoted or unsupported function names are forbidden".into(),
                );
            }
            let full = parts
                .iter()
                .map(|part| part.value.to_lowercase())
                .collect::<Vec<_>>()
                .join(".");
            let name = full.rsplit('.').next().unwrap_or(&full);
            let denied = match self.source.kind {
                DatabaseKind::Clickhouse => CH_DENY.contains(&name),
                _ => PG_DENY.contains(&name),
            };
            if denied {
                return ControlFlow::Break(format!("function {full} is forbidden"));
            }
        }
        ControlFlow::Continue(())
    }
}

fn check_set_expr(body: &SetExpr) -> std::result::Result<(), &'static str> {
    match body {
        SetExpr::Select(select) if select.into.is_none() => Ok(()),
        SetExpr::Select(_) => Err("SELECT INTO is forbidden"),
        SetExpr::Query(query) => check_set_expr(&query.body),
        SetExpr::SetOperation { left, right, .. } => {
            check_set_expr(left)?;
            check_set_expr(right)
        }
        _ => Err("only SELECT query bodies are allowed"),
    }
}

const CH_DENY: &[&str] = &[
    "url",
    "file",
    "s3",
    "s3cluster",
    "remote",
    "remotesecure",
    "mysql",
    "postgresql",
    "jdbc",
    "odbc",
    "hdfs",
    "hdfscluster",
    "azureblobstorage",
    "cluster",
    "clusterallreplicas",
    "input",
    "dictget",
    "dictgetornull",
    "dictgetordefault",
    "dictgetall",
    "dicthas",
    "addresstoline",
    "addresstosymbol",
];
const PG_DENY: &[&str] = &[
    "pg_read_file",
    "pg_read_binary_file",
    "pg_ls_dir",
    "pg_ls_logdir",
    "pg_ls_waldir",
    "lo_import",
    "lo_export",
    "lo_get",
    "lo_put",
    "lo_create",
    "lo_unlink",
    "dblink",
    "dblink_exec",
    "pg_sleep",
    "pg_advisory_lock",
    "pg_advisory_xact_lock",
    "pg_try_advisory_lock",
    "pg_try_advisory_xact_lock",
    "pg_logical_emit_message",
    "set_config",
    "pg_reload_conf",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "current_setting",
    "query_to_xml",
    "query_to_json",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SecretRef;
    fn source(kind: DatabaseKind) -> Source {
        Source {
            id: "test".into(),
            kind,
            host: "localhost".into(),
            port: 5432,
            database: "db".into(),
            user: "reader".into(),
            user_secret: None,
            password: SecretRef::Env {
                name: "TEST_PASSWORD".into(),
            },
            allow_insecure: true,
            ca_file: None,
            max_connections: 2,
            timeout_seconds: 5,
            max_rows: 10,
            allowed_schemas: vec!["public".into()],
            allowed_tables: vec![],
        }
    }
    #[test]
    fn rejects_writes_nested_tables_and_escape_functions() {
        let pg = source(DatabaseKind::Postgres);
        assert!(validate(&pg, "DELETE FROM public.t", None).is_err());
        assert!(validate(&pg, "SELECT * FROM public.t; DROP TABLE public.t", None).is_err());
        assert!(
            validate(
                &pg,
                "SELECT * FROM public.t WHERE id IN (SELECT id FROM secret)",
                None
            )
            .is_err()
        );
        assert!(validate(&pg, "SELECT pg_catalog.pg_read_file('/etc/passwd')", None).is_err());
        assert!(
            validate(
                &pg,
                "SELECT pg_catalog.\"pg_read_file\"('/etc/passwd')",
                None
            )
            .is_err()
        );
        assert!(validate(&pg, "SELECT pg_advisory_lock(1)", None).is_err());
        assert!(validate(&pg, "SELECT * INTO public.copy FROM public.t", None).is_err());
        assert!(validate(&pg, "SELECT * FROM \"public.t\"", None).is_err());
        assert!(validate(&pg, "SELECT * FROM t", None).is_err());
        let ch = source(DatabaseKind::Clickhouse);
        assert!(
            validate(
                &ch,
                "SELECT * FROM s3('https://example.org/a', 'CSV')",
                None
            )
            .is_err()
        );
        assert!(validate(&ch, "SELECT * FROM public.t SETTINGS readonly=0", None).is_err());
        let mut ch_exact = ch.clone();
        ch_exact.allowed_schemas.clear();
        ch_exact.allowed_tables = vec!["public.Secret".into()];
        assert!(validate(&ch_exact, "SELECT * FROM public.Secret", None).is_ok());
        assert!(validate(&ch_exact, "SELECT * FROM public.secret", None).is_err());
    }
    #[test]
    fn scopes_ctes_and_caps_existing_limit() {
        let pg = source(DatabaseKind::Postgres);
        let accepted = validate(
            &pg,
            "WITH x AS (SELECT id FROM public.t) SELECT id FROM x LIMIT 9999",
            Some(4),
        )
        .unwrap();
        assert_eq!(accepted.tables, vec!["public.t"]);
        assert!(accepted.sql.ends_with("LIMIT 5"));
    }
}
