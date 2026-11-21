/// `xxd`-like hex dump with colored output.
pub fn dump_colored(buf: &[u8]) {
    use std::fmt::Write as _;

    const ROW_WIDTH: usize = 16;

    for (index, row) in buf.chunks(ROW_WIDTH).enumerate() {
        let offset = index * ROW_WIDTH;
        let mut line = format!("{offset:08x}: ");
        for column in 0..ROW_WIDTH {
            if let Some(&byte) = row.get(column) {
                write!(line, "{}", colored_hex(byte)).unwrap();
            } else {
                line.push_str("  ");
            }
            if column % 2 == 1 {
                line.push(' ');
            }
        }
        line.push(' ');
        for &byte in row {
            write!(line, "{}", colored_char(byte)).unwrap();
        }
        println!("{line}");
    }
}

fn colored_char(byte: u8) -> colored::ColoredString {
    use colored::Colorize;
    let display = if (b' '..=b'~').contains(&byte) {
        char::from(byte)
    } else {
        '.'
    };
    display.to_string().color(color_for(byte)).bold()
}

fn colored_hex(byte: u8) -> colored::ColoredString {
    use colored::Colorize;
    format!("{byte:02x}").color(color_for(byte)).bold()
}

fn color_for(byte: u8) -> colored::Color {
    use colored::Color;

    match byte {
        0x00 => Color::White,
        0xff => Color::Blue,
        b'\t' | b'\n' | b'\r' => Color::Yellow,
        b' '..=b'~' => Color::Green,
        _ => Color::Red,
    }
}
