use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let input_path = if args.len() > 1 { &args[1] } else { "../Client/extracted/AndroidManifest.xml" };
    let output_path = if args.len() > 2 { &args[2] } else { "../Client/extracted/AndroidManifest_patched.xml" };

    println!("Patching AXML: {} -> {}", input_path, output_path);
    let data = fs::read(input_path)?;

    let patched = patch_axml(&data, &[
        ("VirtualDesktop.Quest.mono.MonoRuntimeProvider.__mono_init__", "org.nebulynx.aperture.mono.MonoRuntimeProvider.__mono_init__"),
        ("VirtualDesktop.Android.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION", "org.nebulynx.aperture.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION"),
        ("VirtualDesktop.Android.androidx-startup", "org.nebulynx.aperture.androidx-startup"),
        ("VirtualDesktop.Quest", "org.nebulynx.aperture"),
        ("Virtual Desktop Quest", "Prism Aperture"),
        ("Virtual Desktop", "Prism Aperture"),
    ])?;

    fs::write(output_path, &patched)?;
    println!("Successfully wrote patched AXML ({} bytes)", patched.len());

    Ok(())
}

fn patch_axml(data: &[u8], replacements: &[(&str, &str)]) -> Result<Vec<u8>, String> {
    if data.len() < 36 {
        return Err("AXML data too small".to_string());
    }

    // Verify XML header
    let xml_type = u16::from_le_bytes([data[0], data[1]]);
    if xml_type != 0x0003 && xml_type != 0x0002 {
        return Err(format!("Invalid XML/Table chunk type: 0x{:04x}", xml_type));
    }
    let xml_header_size = u16::from_le_bytes([data[2], data[3]]) as usize;
    let original_xml_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

    // String pool starts right after XML header (usually offset 8)
    let sp_offset = xml_header_size;
    let sp_type = u16::from_le_bytes([data[sp_offset], data[sp_offset + 1]]);
    if sp_type != 0x0001 {
        return Err(format!("Invalid String Pool type: 0x{:04x}", sp_type));
    }
    let sp_header_size = u16::from_le_bytes([data[sp_offset + 2], data[sp_offset + 3]]) as usize;
    let sp_chunk_size = u32::from_le_bytes([
        data[sp_offset + 4],
        data[sp_offset + 5],
        data[sp_offset + 6],
        data[sp_offset + 7],
    ]) as usize;

    let string_count = u32::from_le_bytes([
        data[sp_offset + 8],
        data[sp_offset + 9],
        data[sp_offset + 10],
        data[sp_offset + 11],
    ]) as usize;
    let style_count = u32::from_le_bytes([
        data[sp_offset + 12],
        data[sp_offset + 13],
        data[sp_offset + 14],
        data[sp_offset + 15],
    ]) as usize;
    let flags = u32::from_le_bytes([
        data[sp_offset + 16],
        data[sp_offset + 17],
        data[sp_offset + 18],
        data[sp_offset + 19],
    ]);
    let strings_start = u32::from_le_bytes([
        data[sp_offset + 20],
        data[sp_offset + 21],
        data[sp_offset + 22],
        data[sp_offset + 23],
    ]) as usize;
    let styles_start = u32::from_le_bytes([
        data[sp_offset + 24],
        data[sp_offset + 25],
        data[sp_offset + 26],
        data[sp_offset + 27],
    ]) as usize;

    let is_utf8 = (flags & (1 << 8)) != 0;
    println!("String pool: count={}, style_count={}, is_utf8={}, strings_start={}",
             string_count, style_count, is_utf8, strings_start);

    // Read string offsets
    let offsets_start = sp_offset + sp_header_size;
    let mut string_offsets = Vec::with_capacity(string_count);
    for i in 0..string_count {
        let o_pos = offsets_start + i * 4;
        let off = u32::from_le_bytes([data[o_pos], data[o_pos + 1], data[o_pos + 2], data[o_pos + 3]]) as usize;
        string_offsets.push(off);
    }

    // Read all strings
    let strings_abs_start = sp_offset + strings_start;
    let mut strings = Vec::with_capacity(string_count);
    for i in 0..string_count {
        let s_pos = strings_abs_start + string_offsets[i];
        if is_utf8 {
            // UTF-8 string format:
            // 1-2 bytes char count, 1-2 bytes byte count, UTF-8 data, 0x00 null
            let mut p = s_pos;
            let mut char_len = data[p] as usize;
            if (data[p] & 0x80) != 0 {
                char_len = (((data[p] & 0x7f) as usize) << 8) | (data[p + 1] as usize);
                p += 2;
            } else {
                p += 1;
            }
            let mut byte_len = data[p] as usize;
            if (data[p] & 0x80) != 0 {
                byte_len = (((data[p] & 0x7f) as usize) << 8) | (data[p + 1] as usize);
                p += 2;
            } else {
                p += 1;
            }
            let str_bytes = &data[p..p + byte_len];
            let s = String::from_utf8_lossy(str_bytes).to_string();
            strings.push(s);
        } else {
            // UTF-16 string format:
            // 2 or 4 bytes char count, UTF-16LE data, 0x0000 null
            let mut p = s_pos;
            let mut char_len = u16::from_le_bytes([data[p], data[p + 1]]) as usize;
            if (char_len & 0x8000) != 0 {
                let high = char_len & 0x7fff;
                let low = u16::from_le_bytes([data[p + 2], data[p + 3]]) as usize;
                char_len = (high << 16) | low;
                p += 4;
            } else {
                p += 2;
            }
            let u16_slice: Vec<u16> = (0..char_len)
                .map(|j| u16::from_le_bytes([data[p + j * 2], data[p + j * 2 + 1]]))
                .collect();
            let s = String::from_utf16_lossy(&u16_slice);
            strings.push(s);
        }
    }

    // Apply replacements
    let mut modified = 0;
    for s in &mut strings {
        for (target, repl) in replacements {
            if s.contains(target) {
                println!("  Replacing in string: '{}' -> target '{}' with '{}'", s, target, repl);
                *s = s.replace(target, repl);
                modified += 1;
            }
        }
    }
    println!("Applied {} replacements in string pool", modified);

    // Re-encode string pool
    let mut new_string_data = Vec::new();
    let mut new_string_offsets = Vec::with_capacity(string_count);

    for s in &strings {
        new_string_offsets.push(new_string_data.len() as u32);
        if is_utf8 {
            let utf8_bytes = s.as_bytes();
            let char_len = s.chars().count();
            if char_len > 0x7f {
                new_string_data.push(((char_len >> 8) as u8) | 0x80);
                new_string_data.push((char_len & 0xff) as u8);
            } else {
                new_string_data.push(char_len as u8);
            }
            let byte_len = utf8_bytes.len();
            if byte_len > 0x7f {
                new_string_data.push(((byte_len >> 8) as u8) | 0x80);
                new_string_data.push((byte_len & 0xff) as u8);
            } else {
                new_string_data.push(byte_len as u8);
            }
            new_string_data.extend_from_slice(utf8_bytes);
            new_string_data.push(0); // null terminator
        } else {
            let u16_chars: Vec<u16> = s.encode_utf16().collect();
            let char_len = u16_chars.len();
            if char_len > 0x7fff {
                let high = ((char_len >> 16) as u16) | 0x8000;
                let low = (char_len & 0xffff) as u16;
                new_string_data.extend_from_slice(&high.to_le_bytes());
                new_string_data.extend_from_slice(&low.to_le_bytes());
            } else {
                new_string_data.extend_from_slice(&(char_len as u16).to_le_bytes());
            }
            for ch in u16_chars {
                new_string_data.extend_from_slice(&ch.to_le_bytes());
            }
            new_string_data.extend_from_slice(&0u16.to_le_bytes()); // null terminator
        }
    }

    // Align string data to 4-byte boundary
    while new_string_data.len() % 4 != 0 {
        new_string_data.push(0);
    }

    // Preserve styles if present
    let styles_data = if style_count > 0 && styles_start > 0 {
        let styles_abs_start = sp_offset + styles_start;
        let styles_abs_end = sp_offset + sp_chunk_size;
        data[styles_abs_start..styles_abs_end].to_vec()
    } else {
        Vec::new()
    };

    let new_styles_start = if style_count > 0 {
        (strings_start + new_string_data.len()) as u32
    } else {
        0
    };

    let new_sp_chunk_size = (strings_start + new_string_data.len() + styles_data.len()) as u32;

    // Assemble new String Pool
    let mut new_sp = Vec::new();
    new_sp.extend_from_slice(&(sp_type as u16).to_le_bytes());
    new_sp.extend_from_slice(&(sp_header_size as u16).to_le_bytes());
    new_sp.extend_from_slice(&new_sp_chunk_size.to_le_bytes());
    new_sp.extend_from_slice(&(string_count as u32).to_le_bytes());
    new_sp.extend_from_slice(&(style_count as u32).to_le_bytes());
    new_sp.extend_from_slice(&flags.to_le_bytes());
    new_sp.extend_from_slice(&(strings_start as u32).to_le_bytes());
    new_sp.extend_from_slice(&new_styles_start.to_le_bytes());

    for off in new_string_offsets {
        new_sp.extend_from_slice(&off.to_le_bytes());
    }

    // Style offsets if any
    if style_count > 0 {
        let style_offsets_start = offsets_start + string_count * 4;
        let style_offsets_bytes = &data[style_offsets_start..offsets_start + (string_count + style_count) * 4];
        new_sp.extend_from_slice(style_offsets_bytes);
    }

    // Pad before string data if needed to match strings_start
    while new_sp.len() < strings_start {
        new_sp.push(0);
    }

    new_sp.extend_from_slice(&new_string_data);
    new_sp.extend_from_slice(&styles_data);

    // Calculate delta and assemble entire output AXML
    let delta = (new_sp.len() as isize) - (sp_chunk_size as isize);
    let new_total_xml_size = ((original_xml_size as isize) + delta) as u32;

    let mut out = Vec::new();
    // Header
    out.extend_from_slice(&(xml_type as u16).to_le_bytes());
    out.extend_from_slice(&(xml_header_size as u16).to_le_bytes());
    out.extend_from_slice(&new_total_xml_size.to_le_bytes());
    if xml_header_size > 8 {
        out.extend_from_slice(&data[8..xml_header_size]);
    }

    // Patched String Pool
    out.extend_from_slice(&new_sp);

    // Remaining chunks from original AXML/ARSC
    let remaining_start = sp_offset + sp_chunk_size;
    let mut remaining = data[remaining_start..].to_vec();

    // If ARSC table, also update package name in ResTable_package if present
    if xml_type == 0x0002 {
        let old_pkg: Vec<u8> = "VirtualDesktop.Quest".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let mut new_pkg: Vec<u8> = "org.nebulynx.aperture".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        while new_pkg.len() < 256 {
            new_pkg.push(0);
        }
        if let Some(pos) = remaining.windows(old_pkg.len()).position(|w| w == old_pkg.as_slice()) {
            println!("  Found package name in ARSC package chunk at offset {}, replacing with org.nebulynx.aperture", pos);
            let end = pos + 256.min(remaining.len() - pos);
            for (i, b) in new_pkg.iter().take(end - pos).enumerate() {
                remaining[pos + i] = *b;
            }
        }
    }

    out.extend_from_slice(&remaining);

    Ok(out)
}
