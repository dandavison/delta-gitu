fn load(p: &Path) -> Cfg {
    // parse the file
    let c = parse(&read(p), Strict, None);
    check(&c);
    c
}
