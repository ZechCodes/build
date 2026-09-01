use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

pub const SHELL_TAIL_LINES: usize = 20;

const MOST_CHARS_IN_A_TAIL_LINE: usize = 2 * 1024;
const CLIPPED_LINE_MARK: char = '…';

const SEEK_BACK_CHUNK_BYTES: u64 = 8 * 1024;
const MOST_BYTES_READ_WHILE_SEEKING_BACK: usize = 256 * 1024;

const EXIT_MARKER_OPENING: &str = "[exited with code ";
const EXIT_MARKER_CLOSING: char = ']';

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellTail {
    pub lines: Vec<String>,
    pub exit_code: Option<i32>,
}

impl ShellTail {
    pub fn read(path: &Path) -> Result<ShellTail, io::Error> {
        let mut file = File::open(path).map_err(|why| failure_naming_the_path(path, why))?;
        let length = file
            .metadata()
            .map_err(|why| failure_naming_the_path(path, why))?
            .len();
        let (held, reached_the_start) = read_back_from_the_end(&mut file, length)
            .map_err(|why| failure_naming_the_path(path, why))?;
        Ok(tail_of(&String::from_utf8_lossy(&held), reached_the_start))
    }
}

fn failure_naming_the_path(path: &Path, why: io::Error) -> io::Error {
    io::Error::new(why.kind(), format!("{}: {why}", path.display()))
}

fn read_back_from_the_end(file: &mut File, length: u64) -> Result<(Vec<u8>, bool), io::Error> {
    let mut held: Vec<u8> = Vec::new();
    let mut unread_before = length;
    while unread_before > 0
        && line_breaks_in(&held) <= SHELL_TAIL_LINES
        && held.len() < MOST_BYTES_READ_WHILE_SEEKING_BACK
    {
        let chunk_starts_at = unread_before.saturating_sub(SEEK_BACK_CHUNK_BYTES);
        let mut chunk = vec![0u8; (unread_before - chunk_starts_at) as usize];
        file.seek(SeekFrom::Start(chunk_starts_at))?;
        file.read_exact(&mut chunk)?;
        chunk.extend_from_slice(&held);
        held = chunk;
        unread_before = chunk_starts_at;
    }
    Ok((held, unread_before == 0))
}

fn line_breaks_in(held: &[u8]) -> usize {
    held.iter().filter(|byte| **byte == b'\n').count()
}

fn tail_of(read_back: &str, reached_the_start: bool) -> ShellTail {
    let mut lines: Vec<String> = read_back.lines().map(str::to_string).collect();
    if !reached_the_start && lines.len() > 1 {
        lines.remove(0);
    }
    if lines.len() > SHELL_TAIL_LINES {
        lines = lines.split_off(lines.len() - SHELL_TAIL_LINES);
    }
    let exit_code = lines.last().and_then(|last| exit_code_marked_by(last));
    ShellTail {
        lines: lines.iter().map(|line| clipped(line)).collect(),
        exit_code,
    }
}

fn clipped(line: &str) -> String {
    match line.chars().count() <= MOST_CHARS_IN_A_TAIL_LINE {
        true => line.to_string(),
        false => {
            let mut kept: String = line.chars().take(MOST_CHARS_IN_A_TAIL_LINE).collect();
            kept.push(CLIPPED_LINE_MARK);
            kept
        }
    }
}

fn exit_code_marked_by(line: &str) -> Option<i32> {
    let up_to_the_close = line.trim_end().strip_suffix(EXIT_MARKER_CLOSING)?;
    let (_, stated) = up_to_the_close.rsplit_once(EXIT_MARKER_OPENING)?;
    stated.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_holding(text: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::tempdir().expect("a temp directory");
        let path = directory.path().join("shell.output");
        std::fs::write(&path, text).expect("the output file writes");
        (directory, path)
    }

    fn hundred_numbered_lines() -> String {
        (1..=100)
            .map(|number| format!("line {number}\n"))
            .collect::<String>()
    }

    #[test]
    fn a_long_file_comes_back_as_its_last_twenty_lines_in_file_order() {
        let (_directory, path) = file_holding(&hundred_numbered_lines());

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.lines.len(), SHELL_TAIL_LINES);
        assert_eq!(tailed.lines.first().map(String::as_str), Some("line 81"));
        assert_eq!(tailed.lines.last().map(String::as_str), Some("line 100"));
        assert_eq!(tailed.exit_code, None);
    }

    fn lines_far_wider_than_one_seek_chunk() -> String {
        (1..=2000)
            .map(|number| format!("line {number} {}\n", "padding".repeat(27)))
            .collect::<String>()
    }

    #[test]
    fn a_file_far_past_one_seek_chunk_comes_back_as_its_last_twenty_lines_in_file_order() {
        let written = lines_far_wider_than_one_seek_chunk();
        assert!(written.len() as u64 > SEEK_BACK_CHUNK_BYTES * 4);
        let (_directory, path) = file_holding(&written);

        let tailed = ShellTail::read(&path).expect("the output file reads");

        let last_twenty: Vec<String> = written
            .lines()
            .skip(2000 - SHELL_TAIL_LINES)
            .map(str::to_string)
            .collect();
        assert_eq!(tailed.lines, last_twenty);
    }

    #[test]
    fn a_chunk_boundary_landing_on_a_line_start_loses_no_line() {
        let (_directory, path) = file_holding(&"abcdefg\n".repeat(2048));

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.lines.len(), SHELL_TAIL_LINES);
        assert!(
            tailed.lines.iter().all(|line| line == "abcdefg"),
            "no line came back cut short: {:?}",
            tailed.lines
        );
    }

    #[test]
    fn the_marker_ending_a_file_far_past_one_seek_chunk_is_the_exit_code() {
        let (_directory, path) = file_holding(&format!(
            "{}[exited with code 7]\n",
            lines_far_wider_than_one_seek_chunk()
        ));

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.exit_code, Some(7));
        assert_eq!(tailed.lines.len(), SHELL_TAIL_LINES);
        assert_eq!(
            tailed.lines.last().map(String::as_str),
            Some("[exited with code 7]")
        );
    }

    #[test]
    fn a_file_shorter_than_the_cap_comes_back_whole() {
        let (_directory, path) = file_holding("tick 1\ntick 2\ntick 3\n");

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.lines, vec!["tick 1", "tick 2", "tick 3"]);
    }

    #[test]
    fn an_empty_file_reads_as_no_lines_and_no_exit_code() {
        let (_directory, path) = file_holding("");

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed, ShellTail::default());
    }

    #[test]
    fn the_marker_at_the_end_is_the_exit_code() {
        let (_directory, path) = file_holding("tick 5\nfinished\n\n[exited with code 3]\n");

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.exit_code, Some(3));
        assert_eq!(
            tailed.lines.last().map(String::as_str),
            Some("[exited with code 3]")
        );
    }

    #[test]
    fn a_file_that_is_still_ticking_reports_no_exit_code() {
        let (_directory, path) = file_holding("tick 4\ntick 5\n");

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.exit_code, None);
    }

    #[test]
    fn a_marker_that_is_not_the_last_line_is_not_this_shells_exit() {
        let (_directory, path) = file_holding("[exited with code 3]\ntick 6\ntick 7\n");

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.exit_code, None);
    }

    #[test]
    fn one_line_longer_than_the_whole_byte_cap_still_comes_back() {
        let unbroken = "x".repeat(MOST_BYTES_READ_WHILE_SEEKING_BACK * 2);
        let (_directory, path) = file_holding(&unbroken);

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.lines.len(), 1, "{:?}", tailed.lines);
    }

    #[test]
    fn a_line_far_past_the_per_line_cap_comes_back_clipped() {
        let (_directory, path) = file_holding(&format!(
            "tick 1\n{}\n",
            "y".repeat(MOST_CHARS_IN_A_TAIL_LINE * 4)
        ));

        let tailed = ShellTail::read(&path).expect("the output file reads");

        let last = tailed.lines.last().expect("a last line");
        assert_eq!(last.chars().count(), MOST_CHARS_IN_A_TAIL_LINE + 1);
        assert!(last.ends_with(CLIPPED_LINE_MARK), "{last}");
        assert_eq!(tailed.lines.first().map(String::as_str), Some("tick 1"));
    }

    #[test]
    fn a_marker_on_a_line_far_past_the_per_line_cap_is_still_the_exit_code() {
        let (_directory, path) = file_holding(&format!(
            "{}[exited with code 4]\n",
            "z".repeat(MOST_CHARS_IN_A_TAIL_LINE * 2)
        ));

        let tailed = ShellTail::read(&path).expect("the output file reads");

        assert_eq!(tailed.exit_code, Some(4));
    }

    #[test]
    fn a_path_that_is_not_there_fails_naming_the_path() {
        let directory = tempfile::tempdir().expect("a temp directory");
        let missing = directory.path().join("never-written.output");

        let why = ShellTail::read(&missing).expect_err("a missing output file does not read");

        assert!(
            why.to_string().contains(&missing.display().to_string()),
            "the failure names the path it could not read: {why}"
        );
    }
}
