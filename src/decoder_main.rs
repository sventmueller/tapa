//! The decoding service: launchd starts it from `Contents/XPCServices/Decoder.xpc` in an App Sandbox of its own,
//! without the user's files and without the network, and it turns the picture bytes the app hands it into pixels.

fn main() {
    splat_decoder::service_main();
}
