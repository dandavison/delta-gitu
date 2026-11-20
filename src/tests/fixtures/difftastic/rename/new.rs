fn load(p: &Path, v: bool) -> Cfg {
    // parse the TOML file
    let cfg = parse(
        &read(p),
        Strict,
        None,
    );
    if v {
        check(&cfg);
    }
    cfg
}
