use std::io;

pub fn read_input() -> io::Result<String> {
    // TODO: Handle non-interaction mode by writing a FIFO file to `/tmp/`
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}
