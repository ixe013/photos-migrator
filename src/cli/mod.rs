use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use clap::{Parser, Subcommand};
use console::{style, Emoji};
use dialoguer::{Confirm, Input};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tracing::warn;

use crate::api::auth::AuthManager;
use crate::api::client::GooglePhotosClient;
use crate::api::models::{GoogleClientCredentials, NewMediaItem, SimpleMediaItem};
use crate::db::{DbAlbumRecord, DbFileRecord, MigrationDb};
use crate::metadata::MetadataEngine;
use crate::takeout::matcher::TakeoutScanner;

static SPARKLE: Emoji<'_, '_> = Emoji("✨ ", "");
static CAMERA: Emoji<'_, '_> = Emoji("📷 ", "");
static FOLDER: Emoji<'_, '_> = Emoji("📁 ", "");
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");

#[derive(Parser, Debug)]
#[command(name = "photos-migrator", author, version, about = "A self-contained tool to process Google Takeout archives, embed metadata, and migrate photos and albums to Google Photos.", long_about = None)]
pub struct Cli {
    #[arg(short, long, global = true, help = "Path to custom SQLite state database [default: .photos-migrator.db]")]
    pub db: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    #[command(about = "Configure Google Cloud OAuth credentials and authenticate Google Photos accounts")]
    Auth {
        #[arg(long, help = "Account identifier (e.g. 'target' or 'source')", default_value = "target")]
        account: String,
        #[arg(long, help = "Path to downloaded Google Cloud credentials.json file")]
        credentials: Option<PathBuf>,
    },

    #[command(about = "Process a Takeout archive locally, injecting timestamps, GPS, and face tags into media headers")]
    FixMetadata {
        #[arg(help = "Path to Google Takeout root folder")]
        takeout_dir: PathBuf,
        #[arg(long, help = "Export fixed files to a new directory instead of in-place modification")]
        output_dir: Option<PathBuf>,
        #[arg(long, help = "Prefer original photos over -edited versions")]
        prefer_originals: bool,
        #[arg(long, help = "Process and keep both original and -edited versions")]
        upload_both: bool,
        #[arg(long, help = "Dry-run without writing changes to disk")]
        dry_run: bool,
    },

    #[command(about = "Analyze Takeout directory, deduplicate files with SHA-256, and populate local migration plan")]
    Plan {
        #[arg(help = "Path to Google Takeout root folder")]
        takeout_dir: PathBuf,
        #[arg(long, help = "Prefer original photos over -edited versions")]
        prefer_originals: bool,
    },

    #[command(about = "Upload photos to Google Photos and recreate albums")]
    Upload {
        #[arg(help = "Path to Google Takeout root folder")]
        takeout_dir: PathBuf,
        #[arg(long, help = "Number of concurrent uploads", default_value = "4")]
        concurrency: usize,
        #[arg(long, help = "Account identifier to upload to", default_value = "target")]
        account: String,
    },

    #[command(about = "Display sharing status and export invitation links for recreated albums")]
    ShareReport {
        #[arg(long, help = "Save report to a Markdown file", default_value = "invitation_report.md")]
        output: PathBuf,
    },

    #[command(about = "Guided interactive wizard running the complete migration end-to-end")]
    Migrate {
        #[arg(help = "Path to Google Takeout root folder")]
        takeout_dir: Option<PathBuf>,
        #[arg(long, help = "Number of concurrent uploads", default_value = "4")]
        concurrency: usize,
    },
}

pub async fn run_cli() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let db_path = cli.db.unwrap_or_else(|| PathBuf::from(".photos-migrator.db"));

    match cli.command {
        Commands::Auth { account, credentials } => {
            cmd_auth(&account, credentials.as_deref()).await?;
        }
        Commands::FixMetadata {
            takeout_dir,
            output_dir,
            prefer_originals,
            upload_both: _,
            dry_run,
        } => {
            cmd_fix_metadata(&takeout_dir, output_dir.as_deref(), !prefer_originals, dry_run)?;
        }
        Commands::Plan {
            takeout_dir,
            prefer_originals,
        } => {
            let db = MigrationDb::open(&db_path)?;
            cmd_plan(&takeout_dir, &db, !prefer_originals)?;
        }
        Commands::Upload {
            takeout_dir: _,
            concurrency,
            account,
        } => {
            let db = MigrationDb::open(&db_path)?;
            cmd_upload(&db, &account, concurrency).await?;
        }
        Commands::ShareReport { output } => {
            let db = MigrationDb::open(&db_path)?;
            cmd_share_report(&db, &output)?;
        }
        Commands::Migrate {
            takeout_dir,
            concurrency,
        } => {
            cmd_migrate(takeout_dir.as_deref(), &db_path, concurrency).await?;
        }
    }

    Ok(())
}

async fn cmd_auth(account: &str, creds_file: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let auth_mgr = AuthManager::new().map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    let credentials = if let Some(path) = creds_file {
        println!("Loading credentials from file: {:?}", path);
        let content = fs::read_to_string(path)?;
        if let Ok(c) = serde_json::from_str::<GoogleClientCredentials>(&content) {
            c
        } else {
            // Check Google standard client_secret format
            let val: serde_json::Value = serde_json::from_str(&content)?;
            let installed = val.get("installed").or_else(|| val.get("web"))
                .ok_or("Invalid Google credentials JSON format")?;
            let cid = installed.get("client_id").and_then(|v| v.as_str()).ok_or("Missing client_id")?;
            let sec = installed.get("client_secret").and_then(|v| v.as_str()).ok_or("Missing client_secret")?;
            GoogleClientCredentials {
                client_id: cid.to_string(),
                client_secret: sec.to_string(),
            }
        }
    } else if let Ok(Some(existing)) = auth_mgr.load_client_credentials() {
        println!("Using existing Google Cloud client credentials.");
        existing
    } else {
        println!("{}", style("Google Cloud OAuth Setup").bold().cyan());
        println!("To interact with Google Photos, you need OAuth Client credentials from Google Cloud Console.");
        println!("1. Go to https://console.cloud.google.com");
        println!("2. Enable 'Photos Library API'");
        println!("3. Create OAuth 2.0 Client ID (Application type: Desktop App)");
        println!("4. Enter your Client ID and Client Secret below:\n");

        let client_id: String = Input::new()
            .with_prompt("Client ID")
            .interact_text()?;

        let client_secret: String = Input::new()
            .with_prompt("Client Secret")
            .interact_text()?;

        GoogleClientCredentials {
            client_id: client_id.trim().to_string(),
            client_secret: client_secret.trim().to_string(),
        }
    };

    auth_mgr.login_interactive(account, &credentials).await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

    Ok(())
}

fn cmd_fix_metadata(
    takeout_dir: &Path,
    output_dir: Option<&Path>,
    prefer_edited: bool,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", style("Scanning Takeout directory for media and JSON sidecars...").bold());
    let scanner = TakeoutScanner::new(takeout_dir);
    let (pairs, _) = scanner.scan(prefer_edited);

    println!("Found {} media items to inspect.", pairs.len());
    if dry_run {
        println!("{}", style("[DRY RUN MODE: No files will be modified]").yellow());
    }

    let meta_engine = MetadataEngine::new();
    let pb = ProgressBar::new(pairs.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut modified = 0;
    let mut skipped = 0;

    for pair in pairs {
        let file_name = pair.media_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        pb.set_message(file_name.to_string());

        if let Some(meta) = &pair.metadata {
            if !dry_run {
                let target_path = if let Some(out_root) = output_dir {
                    let rel = pair.media_path.strip_prefix(takeout_dir).unwrap_or(&pair.media_path);
                    let dest = out_root.join(rel);
                    if let Some(p) = dest.parent() {
                        let _ = fs::create_dir_all(p);
                    }
                    let _ = fs::copy(&pair.media_path, &dest);
                    dest
                } else {
                    pair.media_path.clone()
                };

                if let Err(e) = meta_engine.embed_metadata(&target_path, meta) {
                    warn!("Failed to embed metadata in {:?}: {}", target_path, e);
                } else {
                    modified += 1;
                }
            } else {
                modified += 1;
            }
        } else {
            skipped += 1;
        }
        pb.inc(1);
    }

    pb.finish_with_message("Done!");
    println!("\n{} Metadata processing completed.", CHECK);
    println!("  - Embedded metadata: {} files", modified);
    println!("  - Skipped (no sidecar found): {} files", skipped);

    Ok(())
}

fn cmd_plan(
    takeout_dir: &Path,
    db: &MigrationDb,
    prefer_edited: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", style("Analyzing Google Takeout archive and populating migration plan...").bold());
    let scanner = TakeoutScanner::new(takeout_dir);
    let (pairs, album_collaborators) = scanner.scan(prefer_edited);

    println!("Discovered {} files. Calculating SHA-256 hashes for deduplication...", pairs.len());

    let pb = ProgressBar::new(pairs.len() as u64);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap(),
    );

    // Save albums to DB
    for (album, collabs) in &album_collaborators {
        let record = DbAlbumRecord {
            album_name: album.clone(),
            source_collaborators: collabs.clone(),
            google_album_id: None,
            share_token: None,
            shareable_url: None,
            created_at: None,
        };
        db.insert_or_update_album(&record)?;
    }

    let mut unique_hashes = HashMap::new();

    for pair in pairs {
        let file_name = pair.media_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        pb.set_message(file_name.to_string());

        if let Ok(bytes) = fs::read(&pair.media_path) {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            let sha = hex::encode(hasher.finalize());
            let size = bytes.len() as u64;

            unique_hashes.entry(sha.clone()).or_insert(size);

            let record = DbFileRecord {
                sha256: sha.clone(),
                file_path: pair.media_path.to_string_lossy().to_string(),
                file_size: size,
                is_edited_version: pair.is_edited_version,
                metadata_fixed: pair.metadata.is_some(),
                upload_token: None,
                google_media_id: None,
                uploaded_at: None,
                error_message: None,
            };

            db.insert_or_update_file(&record)?;

            if let Some(album) = &pair.album_name {
                db.link_album_item(album, &sha)?;
            }
        }
        pb.inc(1);
    }

    pb.finish_with_message("Plan updated!");

    let stats = db.get_stats()?;
    let albums = db.get_all_albums()?;

    println!("\n{}", style("=== MIGRATION PLAN OVERVIEW ===").bold().cyan());
    println!("  Total Unique Files:      {}", stats.total_files);
    println!("  Already Uploaded:        {}", stats.uploaded_files);
    println!("  Pending Upload:          {}", stats.total_files - stats.uploaded_files);
    println!("  Total Data Size:         {:.2} GB", stats.total_bytes as f64 / 1_073_741_824.0);
    println!("  Albums Found:            {}", stats.total_albums);

    if !albums.is_empty() {
        println!("\n{}", style("Albums detected:").bold());
        for a in &albums {
            let collabs = if a.source_collaborators.is_empty() {
                "private".to_string()
            } else {
                format!("shared with: {}", a.source_collaborators.join(", "))
            };
            println!("  {} {} ({})", FOLDER, a.album_name, style(collabs).dim());
        }
    }

    let est_batch_calls = (stats.total_files - stats.uploaded_files + 49) / 50;
    println!("\n  Estimated API batchCreate calls: {} (Google allows 10,000/day)", est_batch_calls);
    println!("  Ready to run: `photos-migrator upload <TAKEOUT_DIR>`\n");

    Ok(())
}

async fn cmd_upload(
    db: &MigrationDb,
    account: &str,
    concurrency: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let client = Arc::new(GooglePhotosClient::new(account).map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?);
    let pending_files = db.get_pending_uploads()?;

    if pending_files.is_empty() {
        println!("{} All files are already uploaded! Checking album associations...", CHECK);
    } else {
        println!("{} Uploading {} pending media files (concurrency: {})...", CAMERA, pending_files.len(), concurrency);

        let mp = MultiProgress::new();
        let total_pb = mp.add(ProgressBar::new(pending_files.len() as u64));
        total_pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} files uploaded ({eta})")
                .unwrap(),
        );

        let sem = Arc::new(Semaphore::new(concurrency));
        let mut upload_tokens: Vec<(String, String, String)> = Vec::new(); // (sha256, uploadToken, file_name)

        for file_record in pending_files {
            let permit = sem.clone().acquire_owned().await.unwrap();
            let client_clone = client.clone();
            let path = PathBuf::from(&file_record.file_path);
            let sha = file_record.sha256.clone();
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("image.jpg").to_string();

            match client_clone.upload_media_bytes(&path, &file_name).await {
                Ok(token) => {
                    upload_tokens.push((sha, token, file_name));
                }
                Err(e) => {
                    warn!("Upload error on {}: {}", path.display(), e);
                    let _ = db.mark_file_error(&sha, &e);
                }
            }

            drop(permit);
            total_pb.inc(1);

            // Register in batches of 50
            if upload_tokens.len() >= 50 {
                register_batch_items(db, &client, &mut upload_tokens).await?;
            }
        }

        // Register any remaining items
        if !upload_tokens.is_empty() {
            register_batch_items(db, &client, &mut upload_tokens).await?;
        }

        total_pb.finish_with_message("Upload complete!");
    }

    // Now recreate and populate albums
    println!("\n{}", style("Synchronizing albums and sharing permissions...").bold());
    let albums = db.get_all_albums()?;

    for mut album in albums {
        // Step 1: Create album if not yet created on Google Photos
        let album_id = if let Some(id) = album.google_album_id.clone() {
            id
        } else {
            println!("Creating album: [{}]", album.album_name);
            match client.create_album(&album.album_name).await {
                Ok(remote_album) => {
                    album.google_album_id = Some(remote_album.id.clone());
                    let _ = db.insert_or_update_album(&album);
                    remote_album.id
                }
                Err(e) => {
                    warn!("Failed to create album '{}': {}", album.album_name, e);
                    continue;
                }
            }
        };

        // Step 2: Share album if it was shared in source
        if !album.source_collaborators.is_empty() && album.shareable_url.is_none() {
            println!("Enabling link sharing for album: [{}]", album.album_name);
            match client.share_album(&album_id).await {
                Ok(share_info) => {
                    album.share_token = share_info.share_token;
                    album.shareable_url = share_info.shareable_url;
                    let _ = db.insert_or_update_album(&album);
                }
                Err(e) => {
                    warn!("Could not share album '{}' (target policy may restrict link sharing): {}", album.album_name, e);
                }
            }
        }

        // Step 3: Add photos belonging to this album
        let unadded = db.get_unadded_album_media_ids(&album.album_name)?;
        if !unadded.is_empty() {
            println!("Adding {} photos to album [{}]...", unadded.len(), album.album_name);
            let media_ids: Vec<String> = unadded.iter().map(|(_, mid)| mid.clone()).collect();
            if let Ok(()) = client.add_media_to_album(&album_id, &media_ids).await {
                for (sha, _) in unadded {
                    let _ = db.mark_album_item_added(&album.album_name, &sha);
                }
            }
        }
    }

    println!("\n{} Migration synchronization finished successfully!", CHECK);
    println!("Run `photos-migrator share-report` to generate album invite URLs.");

    Ok(())
}

async fn register_batch_items(
    db: &MigrationDb,
    client: &GooglePhotosClient,
    upload_tokens: &mut Vec<(String, String, String)>,
) -> Result<(), Box<dyn std::error::Error>> {
    let items: Vec<NewMediaItem> = upload_tokens
        .iter()
        .map(|(_, token, fname)| NewMediaItem {
            description: None,
            simple_media_item: SimpleMediaItem {
                upload_token: token.clone(),
                file_name: fname.clone(),
            },
        })
        .collect();

    match client.batch_create_media_items(None, items).await {
        Ok(results) => {
            let res_map: HashMap<String, String> = results.into_iter().collect();
            for (sha, token, _) in upload_tokens.drain(..) {
                if let Some(media_id) = res_map.get(&token) {
                    let _ = db.mark_file_uploaded(&sha, media_id);
                }
            }
        }
        Err(e) => {
            warn!("batchCreate failed: {}", e);
            upload_tokens.clear();
        }
    }

    Ok(())
}

fn cmd_share_report(db: &MigrationDb, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let albums = db.get_all_albums()?;
    let stats = db.get_stats()?;

    let mut markdown = String::new();
    markdown.push_str("# Google Photos Migration - Album Sharing Report\n\n");
    markdown.push_str(&format!("*Generated on: {}*\n\n", chrono::Utc::now().to_rfc3339()));
    markdown.push_str("### Summary\n");
    markdown.push_str(&format!("- **Total Photos Migrated**: {}\n", stats.uploaded_files));
    markdown.push_str(&format!("- **Total Albums**: {}\n", stats.total_albums));
    markdown.push_str(&format!("- **Shared Albums Created**: {}\n\n", stats.shared_albums));

    markdown.push_str("### Album Invitation Links\n\n");
    markdown.push_str("| Album Title | Original Collaborators | Google Photos Invite URL |\n");
    markdown.push_str("|---|---|---|\n");

    println!("\n{}", style("=== ALBUM INVITATION LINKS ===").bold().cyan());

    for a in &albums {
        let collabs_str = if a.source_collaborators.is_empty() {
            "None (Private)".to_string()
        } else {
            a.source_collaborators.join(", ")
        };

        let link_str = a.shareable_url.as_deref().unwrap_or("*(Not shared)*");

        markdown.push_str(&format!("| **{}** | {} | [{}]({}) |\n", a.album_name, collabs_str, link_str, link_str));
        println!("📁 **{}**", a.album_name);
        println!("   Collaborators: {}", collabs_str);
        println!("   Invite Link:   {}\n", style(link_str).green());
    }

    fs::write(output, markdown)?;
    println!("{} Report exported to {:?}", CHECK, output);

    Ok(())
}

async fn cmd_migrate(
    takeout_dir: Option<&Path>,
    db_path: &Path,
    concurrency: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", style("==================================================").bold().cyan());
    println!("{}", style("        Google Photos Migrator - Setup Wizard     ").bold().cyan());
    println!("{}", style("==================================================\n").bold().cyan());

    // Step 1: Authentication
    println!("{} Step 1: Authentication", SPARKLE);
    cmd_auth("target", None).await?;

    // Step 2: Select Takeout Directory
    let dir = if let Some(d) = takeout_dir {
        d.to_path_buf()
    } else {
        let input: String = Input::new()
            .with_prompt("Path to Google Takeout archive folder")
            .interact_text()?;
        PathBuf::from(input.trim())
    };

    if !dir.exists() {
        return Err(format!("Directory {:?} does not exist", dir).into());
    }

    // Step 3: Fix Metadata
    println!("\n{} Step 2: Metadata Processing", SPARKLE);
    let fix_confirm = Confirm::new()
        .with_prompt("Fix and embed EXIF date, GPS, and face tags in-place?")
        .default(true)
        .interact()?;

    if fix_confirm {
        cmd_fix_metadata(&dir, None, true, false)?;
    }

    // Step 4: Plan and Deduplicate
    println!("\n{} Step 3: Deduplication and Planning", SPARKLE);
    let db = MigrationDb::open(db_path)?;
    cmd_plan(&dir, &db, true)?;

    // Step 5: Upload
    let upload_confirm = Confirm::new()
        .with_prompt("Proceed with uploading photos and creating albums?")
        .default(true)
        .interact()?;

    if upload_confirm {
        cmd_upload(&db, "target", concurrency).await?;
        let report_path = PathBuf::from("invitation_report.md");
        cmd_share_report(&db, &report_path)?;
    }

    println!("\n{} Migration workflow complete!", CHECK);
    Ok(())
}
