extern crate bindgen;

fn main() {
    let proto_files = ["proto/tunnel.proto3"];

    // Compile & gen protobuf code
    prost_build::compile_protos(&proto_files, &["proto"]).expect("Couldn't build protobuf files");

    proto_files
        .iter()
        .for_each(|x| println!("cargo:rerun-if-changed={}", x));
}
