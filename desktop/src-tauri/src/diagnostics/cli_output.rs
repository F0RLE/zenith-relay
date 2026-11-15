use std::io::Write;

pub(crate) fn write_report(
    output: &serde_json::Value,
    exit_code: i32,
    writer: &mut impl Write,
) -> i32 {
    if serde_json::to_writer(&mut *writer, output).is_err()
        || writer.write_all(b"\n").is_err()
        || writer.flush().is_err()
    {
        return 2;
    }
    exit_code
}
