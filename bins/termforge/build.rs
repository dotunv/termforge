fn main() {
    println!("cargo:rerun-if-changed=../../assets/icons/termforge.ico");

    #[cfg(windows)]
    embed_resource::compile("termforge.rc", embed_resource::NONE)
        .manifest_required()
        .expect("embed TermForge application icon");
}
