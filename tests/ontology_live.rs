use txttsql_mcp::{config::Memory, memory::MemoryStore};

#[tokio::test]
#[ignore = "requires ONTOLOGY_TEST_ROOT pointing to a read-only txttsql/memory tree"]
async fn reads_existing_typed_graph() {
    let store = MemoryStore::new(&Memory {
        enabled: false,
        path: None,
        ontology_path: Some(std::env::var("ONTOLOGY_TEST_ROOT").unwrap().into()),
    });
    let graph = store.graph("ontology", "payments", 2).await.unwrap();
    assert!(graph["nodes"].as_array().unwrap().len() > 1);
    assert!(!graph["edges"].as_array().unwrap().is_empty());
    assert_eq!(graph["truncated"], false);
}
