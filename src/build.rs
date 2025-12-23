use winres;

fn main() {
    let mut res = winres::WindowsResource::new();
    res.set_icon("icon.ico")
    .set("InternalName", "showBPP.exe")
    .set("OriginalFilename", "showBPP.exe")
    .set("FileDescription", "视频批量获取BPP工具")
    .set("LegalCopyright", "Copyright © 2026 JARK006")
    .set("ProductName", "showBPP")
    .set("CompanyName", "JARK006")
    .set_language(0x804); // 中文简体 - China
    res.compile().unwrap();
}