use markise_web_render::ShotCache;

#[test]
fn the_least_recently_used_goes_first_and_the_bound_holds() {
    let mut c = ShotCache::new(100);
    c.insert(1, "a", 40);
    c.insert(2, "b", 40);
    assert_eq!(c.get(1), Some(&"a"));
    let gone = c.insert(3, "c", 40);
    assert_eq!(gone, [2], "2 was used longest ago");
    assert!(c.contains(1) && c.contains(3) && !c.contains(2));
    assert_eq!(c.total(), 80);
    // peeking changes nothing
    assert_eq!(c.peek(1), Some(&"a"));
    let gone = c.insert(4, "d", 40);
    assert_eq!(gone, [1]);
}

#[test]
fn what_alone_is_over_the_bound_is_not_kept() {
    let mut c = ShotCache::new(10);
    c.insert(1, 1, 5);
    assert_eq!(c.insert(2, 2, 11), [2]);
    assert!(c.contains(1) && !c.contains(2));
    assert_eq!(c.total(), 5);
}

#[test]
fn a_new_value_replaces_the_old_and_its_cost() {
    let mut c = ShotCache::new(10);
    c.insert(1, 1, 8);
    c.insert(1, 2, 3);
    assert_eq!(c.peek(1), Some(&2));
    assert_eq!(c.total(), 3);
    c.clear();
    assert!(c.is_empty());
    assert_eq!(c.total(), 0);
}
