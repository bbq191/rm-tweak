//! Form XObject 自引用/深嵌套不能把栈打爆（fork 改动 ⑤）。
use lopdf::content::{Content, Operation};
use lopdf::{dictionary, Document, Object, Stream};

/// 一页，页面内容 `Do /F`；`/F` 是一个 Form XObject，它自己的内容也是 `Do /F`、资源里的 `/F` 指回自己。
fn self_referencing_form_pdf() -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let form_id = doc.new_object_id();
    let do_f = Content { operations: vec![Operation::new("Do", vec![Object::Name(b"F".to_vec())])] }.encode().unwrap();
    let form = Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Form",
            "BBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
            "Resources" => dictionary! { "XObject" => dictionary! { "F" => form_id } },
        },
        do_f.clone(),
    );
    doc.objects.insert(form_id, Object::Stream(form));
    let content_id = doc.add_object(Stream::new(dictionary! {}, do_f));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "F" => form_id } },
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 100.into(), 100.into()],
        }),
    );
    let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog_id);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[test]
fn self_referencing_form_xobject_does_not_overflow_stack() {
    let pdf = self_referencing_form_pdf();
    let text = pdf_extract_cj::extract_text_from_mem(&pdf).expect("应正常返回而不是栈溢出");
    assert!(text.trim().is_empty(), "{:?}", text);
}
