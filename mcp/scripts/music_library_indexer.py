#!/usr/bin/env python3
"""Music/SFX library indexer — scrapes YouTube channels and Pixabay for copyright-free audio.

Builds a de-duplicated JSON index with:
- filename (sanitized title)
- tags (from title/description keywords)
- download_url (direct audio URL or YouTube video ID)
- source (youtube/pixabay/taketones)
- duration_s
- type (music/sfx)
- license

Usage:
    python3 music_library_indexer.py --build    # Build full index from all sources
    python3 music_library_indexer.py --search "epic battle"  # Search index
"""

import argparse
import json
import os
import re
import sys
import subprocess
from pathlib import Path


# Source channels for copyright-free music
MUSIC_SOURCES = [
    {"name": "NoCopyrightSounds", "url": "https://www.youtube.com/@NoCopyrightSounds", "type": "music"},
    {"name": "AudioLibrary", "url": "https://www.youtube.com/channel/UCQsBfyc5eOobgCzeY8bBzFg", "type": "music"},
    {"name": "BreakingCopyright", "url": "https://www.youtube.com/@BreakingCopyright", "type": "music"},
    {"name": "VlogNoCopyrightMusic", "url": "https://www.youtube.com/@VlogNoCopyrightMusic", "type": "music"},
    {"name": "MixtureOfficial", "url": "https://www.youtube.com/channel/UCkRrhwhJ2Ia_ZlkTQ4XFWJA", "type": "music"},
]

# Source channels for sound effects
SFX_SOURCES = [
    {"name": "SoundLibrary1", "url": "https://www.youtube.com/@SoundLibrary1", "type": "sfx"},
    {"name": "YouTubeSoundEffects", "url": "https://www.youtube.com/@youtubesoundeffects2692/videos", "type": "sfx"},
]

# Pixabay music URL (free, no API key for basic search)
PIXABAY_MUSIC_URL = "https://pixabay.com/music/"


def sanitize_filename(title):
    """Remove special characters from a title for use as filename."""
    return re.sub(r'[^a-zA-Z0-9_-]', '_', title)[:80].strip('_')


def extract_tags(title, source_name=""):
    """Extract search tags from a video title."""
    # Remove common prefixes
    title = re.sub(r'\[.*?\]', '', title)
    title = re.sub(r'\(.*?\)', '', title)
    title = re.sub(r'Official.*', '', title, flags=re.IGNORECASE)
    title = re.sub(r'Lyric Video', '', title, flags=re.IGNORECASE)
    title = re.sub(r'Music Video', '', title, flags=re.IGNORECASE)
    
    # Split into words, lowercase, filter short words
    words = [w.lower().strip('.,!?|') for w in title.split()]
    tags = [w for w in words if len(w) > 2 and w not in {'the', 'and', 'for', 'with', 'your', 'you', 'are', 'but', 'not', 'all', 'can', 'has', 'this', 'that'}]
    
    # Add source name as a tag
    if source_name:
        tags.append(source_name.lower())
    
    return tags


def scrape_youtube_channel(channel_url, source_name, media_type, max_videos=50):
    """Scrape video titles and IDs from a YouTube channel using yt-dlp."""
    print(f"Scraping {source_name} ({channel_url})...", file=sys.stderr)
    
    try:
        result = subprocess.run([
            "yt-dlp",
            "--flat-playlist",
            "--dump-json",
            "--no-warnings",
            "--playlist-end", str(max_videos),
            channel_url
        ], capture_output=True, text=True, timeout=120)
        
        entries = []
        for line in result.stdout.strip().split('\n'):
            if not line:
                continue
            try:
                entry = json.loads(line)
                title = entry.get("title", "")
                video_id = entry.get("id", "")
                duration = entry.get("duration", 0) or 0
                
                if not title or not video_id:
                    continue
                
                # Skip non-music content (interviews, announcements, etc.)
                lower_title = title.lower()
                if any(skip in lower_title for skip in ['interview', 'announcement', 'q&a', 'faq', 'subscribe', 'follow me']):
                    continue
                
                filename = sanitize_filename(title)
                tags = extract_tags(title, source_name)
                
                entries.append({
                    "filename": f"{filename}.mp3",
                    "title": title,
                    "tags": tags,
                    "download_url": f"https://www.youtube.com/watch?v={video_id}",
                    "video_id": video_id,
                    "source": source_name,
                    "source_type": "youtube",
                    "media_type": media_type,
                    "duration_s": duration,
                    "license": "youtube-creative-commons" if "creative commons" in lower_title else "no-copyright",
                })
            except json.JSONDecodeError:
                continue
        
        print(f"  Found {len(entries)} entries", file=sys.stderr)
        return entries
    except subprocess.TimeoutExpired:
        print(f"  Timeout scraping {source_name}", file=sys.stderr)
        return []
    except Exception as e:
        print(f"  Error scraping {source_name}: {e}", file=sys.stderr)
        return []


def search_pixabay_music(query, limit=20):
    """Search Pixabay for free music (no API key needed for basic search)."""
    print(f"Searching Pixabay for: {query}...", file=sys.stderr)
    # Pixabay doesn't have a public API for music without a key
    # We'll use the web search URL and let yt-dlp handle it
    return []  # Placeholder — Pixabay music API requires authentication


def build_index(output_path="mcp/assets/music_library_index.json"):
    """Build the complete music/SFX library index."""
    all_entries = []
    seen_titles = set()  # For deduplication
    
    # Scrape YouTube music channels
    for source in MUSIC_SOURCES:
        entries = scrape_youtube_channel(source["url"], source["name"], source["type"])
        for entry in entries:
            title_key = entry["title"].lower().strip()
            if title_key not in seen_titles:
                seen_titles.add(title_key)
                all_entries.append(entry)
    
    # Scrape YouTube SFX channels
    for source in SFX_SOURCES:
        entries = scrape_youtube_channel(source["url"], source["name"], source["type"])
        for entry in entries:
            title_key = entry["title"].lower().strip()
            if title_key not in seen_titles:
                seen_titles.add(title_key)
                all_entries.append(entry)
    
    # Add local stock music to the index
    local_music_dir = Path("mcp/assets/music")
    if local_music_dir.exists():
        for mp3_file in local_music_dir.glob("*.mp3"):
            title = mp3_file.stem.replace('_', ' ')
            tags = extract_tags(title, "OpenScript Stock")
            all_entries.append({
                "filename": mp3_file.name,
                "title": title,
                "tags": tags,
                "download_url": str(mp3_file),
                "video_id": "",
                "source": "OpenScript Stock",
                "source_type": "local",
                "media_type": "music",
                "duration_s": 30,
                "license": "openscript-stock",
            })
    
    # Write index
    index = {
        "total_entries": len(all_entries),
        "music_count": sum(1 for e in all_entries if e["media_type"] == "music"),
        "sfx_count": sum(1 for e in all_entries if e["media_type"] == "sfx"),
        "sources": [s["name"] for s in MUSIC_SOURCES + SFX_SOURCES],
        "entries": all_entries,
    }
    
    with open(output_path, "w") as f:
        json.dump(index, f, indent=2)
    
    print(f"\nIndex built: {len(all_entries)} entries ({index['music_count']} music, {index['sfx_count']} SFX)")
    print(f"Saved to: {output_path}")
    return index


def search_index(query, media_type=None, limit=10):
    """Search the library index."""
    index_path = "mcp/assets/music_library_index.json"
    if not os.path.exists(index_path):
        print("Index not found. Run with --build first.", file=sys.stderr)
        return []
    
    with open(index_path) as f:
        index = json.load(f)
    
    query_lower = query.lower()
    query_words = query_lower.split()
    
    results = []
    for entry in index["entries"]:
        if media_type and entry["media_type"] != media_type:
            continue
        
        # Score based on tag matches and title matches
        score = 0
        title_lower = entry["title"].lower()
        tags_lower = [t.lower() for t in entry["tags"]]
        
        # Exact title match
        if query_lower in title_lower:
            score += 10
        
        # Word matches in title
        for word in query_words:
            if word in title_lower:
                score += 3
            if word in tags_lower:
                score += 5
        
        if score > 0:
            results.append({**entry, "relevance_score": score})
    
    # Sort by relevance
    results.sort(key=lambda x: x["relevance_score"], reverse=True)
    return results[:limit]


def download_entry(entry, output_dir="mcp/assets/music_cache"):
    """Download a music/SFX entry using yt-dlp."""
    os.makedirs(output_dir, exist_ok=True)
    
    if entry["source_type"] == "local":
        # Already local
        return entry["download_url"]
    
    output_path = os.path.join(output_dir, entry["filename"])
    if os.path.exists(output_path):
        return output_path  # Already downloaded
    
    # Download with yt-dlp as MP3
    try:
        subprocess.run([
            "yt-dlp",
            "-x", "--audio-format", "mp3",
            "--audio-quality", "0",
            "-o", output_path,
            "--no-playlist",
            "--quiet",
            entry["download_url"]
        ], check=True, timeout=120)
        return output_path
    except Exception as e:
        print(f"Download failed: {e}", file=sys.stderr)
        return None


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Music/SFX Library Indexer")
    parser.add_argument("--build", action="store_true", help="Build the library index")
    parser.add_argument("--search", type=str, help="Search the library")
    parser.add_argument("--type", choices=["music", "sfx"], help="Filter by media type")
    parser.add_argument("--limit", type=int, default=10, help="Max search results")
    parser.add_argument("--download", type=str, help="Download a specific entry by filename")
    
    args = parser.parse_args()
    
    if args.build:
        build_index()
    elif args.search:
        results = search_index(args.search, args.type, args.limit)
        print(json.dumps(results, indent=2))
    elif args.download:
        # Find entry by filename and download
        index_path = "mcp/assets/music_library_index.json"
        with open(index_path) as f:
            index = json.load(f)
        for entry in index["entries"]:
            if entry["filename"] == args.download:
                path = download_entry(entry)
                print(f"Downloaded to: {path}" if path else "Download failed")
                break
    else:
        parser.print_help()
