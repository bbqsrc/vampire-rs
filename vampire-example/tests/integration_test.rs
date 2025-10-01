use vampire;

#[vampire::test]
fn test_from_tests_directory() {
    assert_eq!(2 + 2, 4);
}

#[vampire::test]
async fn test_async_from_tests_directory() {
    tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
    assert!(true);
}
