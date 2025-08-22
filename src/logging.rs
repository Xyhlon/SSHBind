use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Seek, SeekFrom};

use crate::LOG_PATH;

pub fn handle_verbose_logging(verbose: bool, log_level: Option<u64>) -> Result<u64, Box<dyn std::error::Error>> {
    if !verbose {
        return Ok(0);
    }

    let start = match log_level {
        Some(0) => SeekFrom::End(0),
        Some(1) => SeekFrom::Start(0),
        None => SeekFrom::End(0),
        Some(_) => {
            return Err("Unsupported Log-Level".into());
        }
    };
    
    std::fs::create_dir_all(LOG_PATH.parent().ok_or("Parent must exist")?)?;

    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH.as_path())?;
    let start_offset = log_file.seek(start)?;
    Ok(start_offset)
}

pub fn print_logs_from_offset(start_offset: u64) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(LOG_PATH.parent().ok_or("Parent must exist")?)?;
    let mut log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_PATH.as_path())?;
    log_file.seek(SeekFrom::Start(start_offset))?;

    let mut reader = BufReader::new(log_file);
    let mut line = String::new();

    // Read until EOF
    while reader.read_line(&mut line)? > 0 {
        print!("{}", line);
        line.clear();
    }
    Ok(())
}