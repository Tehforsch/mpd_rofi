use clap::{Parser, Subcommand};
use rand::seq::SliceRandom;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Parser)]
#[command(name = "music_selection")]
#[command(about = "Music selection tool")]
struct Cli {
    #[arg(long, help = "Pre-select artist")]
    artist: Option<String>,

    #[arg(long, help = "Pre-select album (requires --artist)")]
    album: Option<String>,

    #[arg(long, default_value = "0", help = "Pre-select song index")]
    preselect: usize,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Select artist then album then song")]
    Artist,
    #[command(about = "Select album then song")]
    Album,
    #[command(about = "Select song from all songs")]
    Song,
    #[command(about = "Play a random album without prompts")]
    Random,
    #[command(about = "Select album from quarantine list")]
    Quarantine,
    #[command(about = "Play a random quarantine album without prompts")]
    RandomQuarantine,
    #[command(about = "Show current playlist and jump to selected song")]
    Playlist,
}

#[derive(Debug, Clone)]
struct Track {
    artist: String,
    album: String,
    title: String,
    track: Option<String>,
    file: String,
}

#[derive(Debug)]
struct MpdClient {
    stream: TcpStream,
}

impl MpdClient {
    fn connect() -> Result<Self, Box<dyn std::error::Error>> {
        let stream = TcpStream::connect("localhost:6600")?;

        // Read initial greeting
        let mut reader = BufReader::new(&stream);
        let mut greeting = String::new();
        reader.read_line(&mut greeting)?;

        if !greeting.starts_with("OK MPD") {
            return Err("Invalid MPD greeting".into());
        }

        Ok(MpdClient { stream })
    }

    fn send_command(&mut self, cmd: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        writeln!(self.stream, "{}", cmd)?;

        let mut reader = BufReader::new(&self.stream);
        let mut lines = Vec::new();
        let mut line = String::new();

        loop {
            line.clear();
            reader.read_line(&mut line)?;
            let trimmed = line.trim();

            if trimmed == "OK" {
                break;
            }
            if trimmed.starts_with("ACK") {
                return Err(format!("MPD error: {}", trimmed).into());
            }

            lines.push(trimmed.to_string());
        }

        Ok(lines)
    }

    fn list_albums(
        &mut self,
        artist: Option<&str>,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        let cmd = if let Some(artist) = artist {
            format!("find albumartist \"{}\"", artist.replace('"', "\\\""))
        } else {
            "listallinfo".to_string()
        };

        let lines = self.send_command(&cmd)?;
        let mut albums = HashSet::new();
        let mut current_artist = String::new();
        let mut current_album = String::new();

        for line in lines {
            if let Some(value) = line.strip_prefix("AlbumArtist: ") {
                current_artist = value.to_string();
            } else if let Some(value) = line.strip_prefix("Album: ") {
                current_album = value.to_string();
            } else if line.starts_with("file: ")
                && !current_artist.is_empty()
                && !current_album.is_empty()
            {
                albums.insert((current_artist.clone(), current_album.clone()));
            }
        }

        Ok(albums.into_iter().collect())
    }

    fn list_artists(&mut self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let lines = self.send_command("list albumartist")?;
        let mut artists = Vec::new();

        for line in lines {
            if let Some(artist) = line.strip_prefix("AlbumArtist: ") {
                if !artist.trim().is_empty() {
                    artists.push(artist.to_string());
                }
            }
        }

        Ok(artists)
    }

    fn list_songs(
        &mut self,
        artist: Option<&str>,
        album: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let cmd = if let (Some(artist), Some(album)) = (artist, album) {
            format!(
                "find albumartist \"{}\" album \"{}\"",
                artist.replace('"', "\\\""),
                album.replace('"', "\\\"")
            )
        } else {
            "listallinfo".to_string()
        };

        let lines = self.send_command(&cmd)?;
        let mut songs = Vec::new();
        let mut current_title = String::new();
        let mut current_artist = String::new();

        for line in lines {
            if let Some(title) = line.strip_prefix("Title: ") {
                current_title = title.to_string();
            } else if let Some(artist) = line.strip_prefix("AlbumArtist: ") {
                current_artist = artist.to_string();
            } else if line.starts_with("file: ") && !current_title.is_empty() {
                if artist.is_none() && album.is_none() {
                    // Return format "artist\ttitle" for all songs
                    songs.push(format!("{}\t{}", current_artist, current_title));
                } else {
                    // Return just title for specific album
                    songs.push(current_title.clone());
                }
                current_title.clear();
                current_artist.clear();
            }
        }

        Ok(songs)
    }

    fn get_playlist(&mut self) -> Result<Vec<Track>, Box<dyn std::error::Error>> {
        let lines = self.send_command("playlistinfo")?;
        let mut tracks = Vec::new();
        let mut current_track = Track {
            artist: String::new(),
            album: String::new(),
            title: String::new(),
            track: None,
            file: String::new(),
        };

        for line in lines {
            if let Some(value) = line.strip_prefix("AlbumArtist: ") {
                current_track.artist = value.to_string();
            } else if let Some(value) = line.strip_prefix("Album: ") {
                current_track.album = value.to_string();
            } else if let Some(value) = line.strip_prefix("Title: ") {
                current_track.title = value.to_string();
            } else if let Some(value) = line.strip_prefix("Track: ") {
                current_track.track = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("file: ") {
                current_track.file = value.to_string();
                tracks.push(current_track.clone());
                current_track = Track {
                    artist: String::new(),
                    album: String::new(),
                    title: String::new(),
                    track: None,
                    file: String::new(),
                };
            }
        }

        Ok(tracks)
    }

    fn get_status(
        &mut self,
    ) -> Result<std::collections::HashMap<String, String>, Box<dyn std::error::Error>> {
        let lines = self.send_command("status")?;
        let mut status = std::collections::HashMap::new();

        for line in lines {
            if let Some(colon_pos) = line.find(": ") {
                let key = line[..colon_pos].to_string();
                let value = line[colon_pos + 2..].to_string();
                status.insert(key, value);
            }
        }

        Ok(status)
    }

    fn find_song_album(
        &mut self,
        artist: &str,
        title: &str,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let cmd = format!(
            "find albumartist \"{}\" title \"{}\"",
            artist.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        let lines = self.send_command(&cmd)?;

        for line in lines {
            if let Some(album) = line.strip_prefix("Album: ") {
                return Ok(Some(album.to_string()));
            }
        }

        Ok(None)
    }
}

struct MusicSelector {
    mpd: MpdClient,
}

impl MusicSelector {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mpd = MpdClient::connect()?;
        Ok(MusicSelector { mpd })
    }

    fn rofi_select(
        &self,
        items: &[String],
        prompt: &str,
        selected_row: usize,
        use_column_formatting: bool,
    ) -> Result<(Option<String>, bool), Box<dyn std::error::Error>> {
        if items.is_empty() {
            return Ok((None, false));
        }

        let input_text = items.join("\n");

        let formatted_input = if use_column_formatting {
            match Command::new("column")
                .args(["-o", "           ", "-s", "\t", "-t"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
            {
                Ok(mut child) => {
                    if let Some(stdin) = child.stdin.take() {
                        let mut stdin = stdin;
                        let _ = stdin.write_all(input_text.as_bytes());
                    }

                    match child.wait_with_output() {
                        Ok(output) if output.status.success() => {
                            String::from_utf8_lossy(&output.stdout).to_string()
                        }
                        _ => input_text,
                    }
                }
                Err(_) => input_text,
            }
        } else {
            input_text
        };

        let mut cmd = Command::new("rofi")
            .args(["-i", "-dmenu", "-no-custom", "-format", "d"])
            .args(["-kb-custom-1", "Ctrl+Return", "-p", prompt])
            .args(["-selected-row", &selected_row.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;

        if let Some(stdin) = cmd.stdin.take() {
            let mut stdin = stdin;
            let _ = stdin.write_all(formatted_input.as_bytes());
        }

        let output = cmd.wait_with_output()?;
        let exit_code = output.status.code().unwrap_or(1);

        if exit_code == 1 {
            return Ok((None, false));
        }

        if let Ok(stdout) = String::from_utf8(output.stdout) {
            let stdout = stdout.trim();
            if !stdout.is_empty() {
                if let Ok(index) = stdout.parse::<usize>() {
                    if index > 0 && index <= items.len() {
                        let selected = items[index - 1].clone();
                        let is_queue = exit_code == 10;
                        return Ok((Some(selected), is_queue));
                    }
                }
            }
        }

        Ok((None, false))
    }

    fn get_artists(&mut self) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        self.mpd.list_artists()
    }

    fn get_albums(
        &mut self,
        artist: Option<&str>,
    ) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        self.mpd.list_albums(artist)
    }

    fn get_songs(
        &mut self,
        artist: Option<&str>,
        album: Option<&str>,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        self.mpd.list_songs(artist, album)
    }

    fn play_song(
        &mut self,
        artist: &str,
        album: Option<&str>,
        title: &str,
        queue_mode: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // If no album provided, find the album that contains this song
        let actual_album = if let Some(album) = album {
            Some(album.to_string())
        } else {
            self.mpd.find_song_album(artist, title)?
        };
        if !queue_mode {
            Command::new("mpc").arg("clear").output()?;

            // Build findadd command - add album filter if we found/have an album
            let mut args = vec!["findadd"];
            if let Some(ref album) = actual_album {
                args.extend_from_slice(&["album", album]);
            }
            args.extend_from_slice(&["albumartist", artist]);

            Command::new("mpc").args(&args).output()?;

            let playlist = Command::new("mpc")
                .args(["playlist", "-f", "%title%"])
                .output()?;

            let playlist_str = String::from_utf8_lossy(&playlist.stdout);
            let songs: Vec<&str> = playlist_str.trim().split('\n').collect();

            if let Some(position) = songs.iter().position(|&s| s == title) {
                Command::new("mpc")
                    .args(["play", &(position + 1).to_string()])
                    .output()?;
                println!(
                    "Playing:\n{}\n{}\n{}",
                    artist,
                    actual_album.as_deref().unwrap_or(""),
                    title
                );
            } else {
                Command::new("mpc").arg("play").output()?;
                println!("Could not find song '{}' in playlist", title);
            }
        } else {
            // Queue the specific song
            let mut args = vec!["findadd", "albumartist", artist];
            if let Some(ref album) = actual_album {
                args.extend_from_slice(&["album", album]);
            }
            args.extend_from_slice(&["title", title]);

            Command::new("mpc").args(&args).output()?;
            println!(
                "Queued:\n{}\n{}\n{}",
                artist,
                actual_album.as_deref().unwrap_or(""),
                title
            );
        }

        Ok(())
    }

    fn show_notification(&self, artist: &str, album: &str, title: Option<&str>) {
        let (summary, message) = if let Some(title) = title {
            ("Now Playing", format!("{}\n{}\n{}", artist, album, title))
        } else {
            ("Now Playing Album", format!("{}\n{}", artist, album))
        };

        let _ = Command::new("notify-send")
            .args(["-t", "3000", summary, &message])
            .output();
    }

    fn play_random_album(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let albums = self.get_albums(None)?;
        if albums.is_empty() {
            println!("No albums found");
            return Ok(());
        }

        let (artist, album) = albums.choose(&mut rand::thread_rng()).unwrap();

        Command::new("mpc").arg("clear").output()?;
        Command::new("mpc")
            .args(["findadd", "album", album, "albumartist", artist])
            .output()?;
        Command::new("mpc").arg("play").output()?;

        println!("Playing random album:\n{}\n{}", artist, album);
        self.show_notification(artist, album, None);

        Ok(())
    }

    fn load_quarantine_albums(&self) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
        let quarantine_path = format!("{}/music/quarantine", std::env::var("HOME")?);

        if !Path::new(&quarantine_path).exists() {
            println!("Quarantine file not found: {}", quarantine_path);
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&quarantine_path)?;
        let re = Regex::new(r#"^"([^"]*)",\s*"([^"]*)"$"#)?;
        let mut albums = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some(captures) = re.captures(line) {
                let artist = captures.get(1).unwrap().as_str().to_string();
                let album = captures.get(2).unwrap().as_str().to_string();
                albums.push((artist, album));
            }
        }

        Ok(albums)
    }

    fn select_quarantine_album(
        &self,
        random_mode: bool,
    ) -> Result<Option<(String, String, bool)>, Box<dyn std::error::Error>> {
        let albums = self.load_quarantine_albums()?;
        if albums.is_empty() {
            println!("No quarantine albums found");
            return Ok(None);
        }

        if random_mode {
            let (artist, album) = albums.choose(&mut rand::thread_rng()).unwrap();
            Ok(Some((artist.clone(), album.clone(), false)))
        } else {
            let tab_separated_items: Vec<String> = albums
                .iter()
                .map(|(artist, album)| format!("{}\t{}", artist, album))
                .collect();

            let (selected_display, queue_mode) =
                self.rofi_select(&tab_separated_items, "Quarantine Album:", 0, true)?;

            if let Some(selected) = selected_display {
                if let Some(index) = tab_separated_items.iter().position(|x| x == &selected) {
                    let (artist, album) = &albums[index];
                    return Ok(Some((artist.clone(), album.clone(), queue_mode)));
                }
            }

            Ok(None)
        }
    }

    fn play_random_quarantine_album(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Some((artist, album, _)) = self.select_quarantine_album(true)? {
            Command::new("mpc").arg("clear").output()?;
            Command::new("mpc")
                .args(["findadd", "album", &album, "albumartist", &artist])
                .output()?;
            Command::new("mpc").arg("play").output()?;

            println!("Playing random quarantine album:\n{}\n{}", artist, album);
            self.show_notification(&artist, &album, None);
        }

        Ok(())
    }

    fn show_playlist(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let playlist = self.mpd.get_playlist()?;
        if playlist.is_empty() {
            println!("Playlist is empty");
            return Ok(());
        }

        let status = self.mpd.get_status()?;
        let current_pos = status
            .get("song")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);

        let playlist_items: Vec<String> = playlist
            .iter()
            .enumerate()
            .map(|(_i, track)| {
                let artist = if track.artist.is_empty() {
                    "Unknown Artist"
                } else {
                    &track.artist
                };
                let title = if track.title.is_empty() {
                    "Unknown Title"
                } else {
                    &track.title
                };

                let display_title = if let Some(track_num) = &track.track {
                    let track_num = track_num.split('/').next().unwrap_or(track_num);
                    if !track_num.is_empty() {
                        format!("{:02} {}", track_num.parse::<u32>().unwrap_or(0), title)
                    } else {
                        title.to_string()
                    }
                } else {
                    title.to_string()
                };

                format!("{}\t{}", artist, display_title)
            })
            .collect();

        let (selected_display, _) =
            self.rofi_select(&playlist_items, "Playlist:", current_pos, true)?;

        if let Some(selected) = selected_display {
            if let Some(index) = playlist_items.iter().position(|x| x == &selected) {
                Command::new("mpc")
                    .args(["play", &(index + 1).to_string()])
                    .output()?;

                let track = &playlist[index];
                let artist = if track.artist.is_empty() {
                    "Unknown Artist"
                } else {
                    &track.artist
                };
                let album = if track.album.is_empty() {
                    "Unknown Album"
                } else {
                    &track.album
                };
                let title = if track.title.is_empty() {
                    "Unknown Title"
                } else {
                    &track.title
                };

                self.show_notification(artist, album, Some(title));
            }
        }

        Ok(())
    }

    fn select_artist(&mut self) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let mut artists = self.get_artists()?;
        if artists.is_empty() {
            println!("No artists found");
            return Ok(None);
        }

        artists.shuffle(&mut rand::thread_rng());
        let (selected, _) = self.rofi_select(&artists, "Artist:", 0, false)?;
        Ok(selected)
    }

    fn select_album(
        &mut self,
        artist: Option<&str>,
    ) -> Result<Option<(String, String, bool)>, Box<dyn std::error::Error>> {
        let mut albums = self.get_albums(artist)?;
        if albums.is_empty() {
            println!("No albums found");
            return Ok(None);
        }

        albums.shuffle(&mut rand::thread_rng());

        if let Some(artist) = artist {
            let album_names: Vec<String> = albums.iter().map(|(_, album)| album.clone()).collect();
            let (selected_album, queue_mode) =
                self.rofi_select(&album_names, "Album:", 0, false)?;
            if let Some(album) = selected_album {
                return Ok(Some((artist.to_string(), album, queue_mode)));
            }
        } else {
            let tab_separated_items: Vec<String> = albums
                .iter()
                .map(|(artist, album)| format!("{}\t{}", artist, album))
                .collect();

            let (selected_display, queue_mode) =
                self.rofi_select(&tab_separated_items, "Album:", 0, true)?;

            if let Some(selected) = selected_display {
                if let Some(index) = tab_separated_items.iter().position(|x| x == &selected) {
                    let (artist, album) = &albums[index];
                    return Ok(Some((artist.clone(), album.clone(), queue_mode)));
                }
            }
        }

        Ok(None)
    }

    fn select_song(
        &mut self,
        artist: Option<&str>,
        album: Option<&str>,
        preselect_index: usize,
    ) -> Result<Option<(String, bool)>, Box<dyn std::error::Error>> {
        let mut songs = self.get_songs(artist, album)?;
        if songs.is_empty() {
            println!("No songs found");
            return Ok(None);
        }

        // Shuffle songs if selecting from all songs
        if artist.is_none() && album.is_none() {
            songs.shuffle(&mut rand::thread_rng());
        }

        let use_column_formatting = artist.is_none() && album.is_none();
        let (selected, queue_mode) = self.rofi_select(
            &songs,
            "Choose a song:",
            preselect_index,
            use_column_formatting,
        )?;
        if let Some(song) = selected {
            return Ok(Some((song, queue_mode)));
        }

        Ok(None)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let mut selector = MusicSelector::new()?;

    match cli.command {
        Some(Commands::Artist) => {
            if let Some(artist) = selector.select_artist()? {
                if let Some((artist, album, queue_mode)) = selector.select_album(Some(&artist))? {
                    if queue_mode {
                        Command::new("mpc")
                            .args(["findadd", "album", &album, "albumartist", &artist])
                            .output()?;
                    } else if let Some((title, song_queue_mode)) =
                        selector.select_song(Some(&artist), Some(&album), cli.preselect)?
                    {
                        selector.play_song(&artist, Some(&album), &title, song_queue_mode)?;
                    }
                }
            }
        }

        Some(Commands::Album) => {
            if let (Some(artist), Some(album)) = (&cli.artist, &cli.album) {
                if let Some((title, queue_mode)) =
                    selector.select_song(Some(artist), Some(album), cli.preselect)?
                {
                    selector.play_song(artist, Some(album), &title, queue_mode)?;
                }
            } else if let Some((artist, album, queue_mode)) =
                selector.select_album(cli.artist.as_deref())?
            {
                if queue_mode {
                    Command::new("mpc")
                        .args(["findadd", "album", &album, "albumartist", &artist])
                        .output()?;
                } else if let Some((title, song_queue_mode)) =
                    selector.select_song(Some(&artist), Some(&album), cli.preselect)?
                {
                    selector.play_song(&artist, Some(&album), &title, song_queue_mode)?;
                }
            }
        }

        Some(Commands::Random) => {
            selector.play_random_album()?;
        }

        Some(Commands::Quarantine) => {
            if let Some((artist, album, queue_mode)) = selector.select_quarantine_album(false)? {
                if queue_mode {
                    Command::new("mpc")
                        .args(["findadd", "album", &album, "albumartist", &artist])
                        .output()?;
                } else if let Some((title, song_queue_mode)) =
                    selector.select_song(Some(&artist), Some(&album), cli.preselect)?
                {
                    selector.play_song(&artist, Some(&album), &title, song_queue_mode)?;
                }
            }
        }

        Some(Commands::RandomQuarantine) => {
            selector.play_random_quarantine_album()?;
        }

        Some(Commands::Playlist) => {
            selector.show_playlist()?;
        }

        Some(Commands::Song) => {
            if let Some((song_result, queue_mode)) =
                selector.select_song(None, None, cli.preselect)?
            {
                if let Some(tab_pos) = song_result.find('\t') {
                    let artist = &song_result[..tab_pos];
                    let title = &song_result[tab_pos + 1..];
                    selector.play_song(artist, None, title, queue_mode)?;
                }
            }
        }

        None => {
            if let Some((artist, album, queue_mode)) = selector.select_album(None)? {
                if queue_mode {
                    Command::new("mpc")
                        .args(["findadd", "album", &album, "albumartist", &artist])
                        .output()?;
                } else if let Some((title, song_queue_mode)) =
                    selector.select_song(Some(&artist), Some(&album), cli.preselect)?
                {
                    selector.play_song(&artist, Some(&album), &title, song_queue_mode)?;
                }
            }
        }
    }

    Ok(())
}
