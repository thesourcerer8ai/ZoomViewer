#!/usr/bin/env perl
# gen_xor_workflow.pl — Generate a ZoomViewer workflow.json for N dump files.
#
# For each unique pair (A,B) of the given input files it creates:
#   Input(A) + Input(B)  →  XOR(A,B)  →  Search(A,B)  →  Concat  →  FileExport
#                                                                    →  OutputViewer
#
# Each distinct filename gets exactly one Input node; all XOR pairs that
# reference the same file share that single Input node.
#
# Usage:
#   perl gen_xor_workflow.pl file1.dump file2.dump [file3.dump ...] > workflow.json
#   perl gen_xor_workflow.pl file1.dump file2.dump -o my_workflow.json
#
# Options:
#   -o <path>   Write output to <path> instead of stdout
#
# Search parameters are copied from the project's canonical workflow.json:
#   search_pattern : "|Block"
#   is_hex         : false
#   case_sensitive : true
#   max_matches    : 3000
#   output_mode    : "AllMatchingPages"
#   bytes_before   : 0
#   bytes_after    : 0

use strict;
use warnings;
use POSIX qw();
use JSON::PP;   # core module since Perl 5.14
use File::Basename qw(basename);
use Getopt::Long qw(GetOptions);

# ── CLI ──────────────────────────────────────────────────────────────────────

my $output_file = '';
GetOptions('o=s' => \$output_file) or die "Usage: $0 [-o output.json] file1 file2 [...]\n";

my @files = @ARGV;
die "Need at least 2 input files.\nUsage: $0 [-o output.json] file1 file2 [...]\n"
    unless @files >= 2;

# ── Metadata helpers ─────────────────────────────────────────────────────────

# Attempt to load page_length + block_size from .cache/<basename>/metadata.json
sub load_metadata {
    my ($path) = @_;
    my $base = basename($path);
    my $meta_path = ".cache/$base/metadata.json";
    if (-f $meta_path) {
        open my $fh, '<', $meta_path or return undef;
        local $/;
        my $json_text = <$fh>;
        close $fh;
        my $data = eval { JSON::PP->new->decode($json_text) };
        if ($data && $data->{page_length} && $data->{block_size}) {
            return ($data->{page_length}, $data->{block_size});
        }
    }
    return undef;
}

# ── Load and validate geometry for all files ─────────────────────────────────

my %geometry;   # file_path => [page_length, block_size]
my $warned_missing = 0;

for my $f (@files) {
    my @geo = load_metadata($f);
    if (@geo && defined $geo[0]) {
        $geometry{$f} = \@geo;
    } else {
        warn "WARNING: No cached metadata found for '$f' — using defaults (page=4096, block=64).\n";
        $geometry{$f} = [4096, 64];
        $warned_missing = 1;
    }
}

# Warn if geometries differ between files
my $ref_file = $files[0];
my ($ref_pl, $ref_bs) = @{$geometry{$ref_file}};
for my $f (@files[1..$#files]) {
    my ($pl, $bs) = @{$geometry{$f}};
    if ($pl != $ref_pl || $bs != $ref_bs) {
        warn "WARNING: Geometry mismatch!\n"
           . "  '$ref_file': page_length=$ref_pl, block_size=$ref_bs\n"
           . "  '$f': page_length=$pl, block_size=$bs\n"
           . "  The XOR operation requires identical geometry. Results may be incorrect.\n";
    }
}

# ── Build node list ───────────────────────────────────────────────────────────
# Layout constants (left-to-right, each column 300px apart)
use constant COL_INPUT   =>    0.0;
use constant COL_XOR     =>  400.0;
use constant COL_SEARCH  =>  800.0;
use constant COL_CONCAT  => 1200.0;
use constant COL_OUTPUT  => 1600.0;
use constant ROW_STEP    =>  180.0;   # vertical spacing between pipeline rows

my @nodes;
my @connections;
my $next_id = 1;

# One Input node per unique file, laid out in a column.
my %input_node_id;   # file_path => node_id
my $input_row = 0;
for my $f (@files) {
    my $id = $next_id++;
    $input_node_id{$f} = $id;
    my ($pl, $bs) = @{$geometry{$f}};
    push @nodes, {
        id   => $id,
        name => basename($f),
        pos  => [ COL_INPUT + 0.0, $input_row * ROW_STEP + 50.0 ],
        kind => { Input => {
            file_path   => $f,
            page_length => $pl + 0,
            block_size  => $bs + 0,
        }},
        status     => "Idle",
        output_log => "No execution history",
    };
    $input_row++;
}

# Generate all unique pairs and build XOR + Search pipelines.
my @concat_input_ids;   # Search node IDs that feed the Concat node
my $pair_row = 0;

for my $i (0 .. $#files - 1) {
    for my $j ($i + 1 .. $#files) {
        my $fa = $files[$i];
        my $fb = $files[$j];
        my $ba = basename($fa);
        my $bb = basename($fb);
        my $y  = $pair_row * ROW_STEP + 50.0;

        # XOR node
        my $xor_id = $next_id++;
        push @nodes, {
            id   => $xor_id,
            name => "XOR $i x $j",
            pos  => [ COL_XOR + 0.0, $y ],
            kind => { XorTransform => { pattern_hex => "" } },
            status     => "Idle",
            output_log => "No execution history",
            collapsed  => JSON::PP::true,
        };
        push @connections, { from_node => $input_node_id{$fa}, to_node => $xor_id };
        push @connections, { from_node => $input_node_id{$fb}, to_node => $xor_id };

        # Search node (parameters copied from the project's canonical workflow.json)
        my $search_id = $next_id++;
        push @nodes, {
            id   => $search_id,
            name => "Search $i x $j",
            pos  => [ COL_SEARCH + 0.0, $y ],
            kind => { PatternSearch => {
                search_pattern => "|Block",
                is_hex         => JSON::PP::false,
                case_sensitive => JSON::PP::true,
                max_matches    => 10000,
                output_mode    => "AllMatchingPages",
                bytes_before   => 0,
                bytes_after    => 0,
                results        => [],
            }},
            status     => "Idle",
            output_log => "No execution history",
            collapsed  => JSON::PP::true,
        };
        push @connections, { from_node => $xor_id, to_node => $search_id };
        push @concat_input_ids, $search_id;

        $pair_row++;
    }
}

# Concat node — centred vertically across all pairs
my $concat_y = (($pair_row - 1) / 2.0) * ROW_STEP + 50.0;
my $concat_id = $next_id++;
push @nodes, {
    id         => $concat_id,
    name       => "Concatenate",
    pos        => [ COL_CONCAT + 0.0, $concat_y ],
    kind       => "Concatenate",
    status     => "Idle",
    output_log => "No execution history",
};
for my $sid (@concat_input_ids) {
    push @connections, { from_node => $sid, to_node => $concat_id };
}

# FileExport node
my $export_id = $next_id++;
push @nodes, {
    id   => $export_id,
    name => "Output Writer",
    pos  => [ COL_OUTPUT + 0.0, $concat_y - ROW_STEP * 0.4 ],
    kind => { FileExport => {
        export_path => "output_combined.bin",
        auto_export => JSON::PP::false,
    }},
    status     => "Idle",
    output_log => "Ready to export",
};
push @connections, { from_node => $concat_id, to_node => $export_id };

# OutputViewer node
my $viewer_id = $next_id++;
push @nodes, {
    id   => $viewer_id,
    name => "Output: Viewer",
    pos  => [ COL_OUTPUT + 0.0, $concat_y + ROW_STEP * 0.4 ],
    kind => { OutputViewer => { name => "Viewer" } },
    status     => "Completed",
    output_log => "Output Viewer 'Viewer' ready and connected to ZoomViewer.",
};
push @connections, { from_node => $concat_id, to_node => $viewer_id };

# ── Assemble and serialise ────────────────────────────────────────────────────

my $workflow = {
    next_node_id   => $next_id,
    nodes          => \@nodes,
    connections    => \@connections,
    selected_node  => $viewer_id,
    connecting_from => undef,
    status_message => sprintf(
        "Generated by gen_xor_workflow.pl: %d input files, %d XOR pairs.",
        scalar @files, scalar @concat_input_ids,
    ),
};

my $json = JSON::PP->new->utf8->pretty->canonical->encode($workflow);

if ($output_file) {
    open my $fh, '>', $output_file
        or die "Cannot write to '$output_file': $!\n";
    print $fh $json;
    close $fh;
    my $n_pairs = scalar @concat_input_ids;
    printf STDERR "Written to '%s'  (%d input files, %d XOR pairs, %d nodes total)\n",
        $output_file, scalar @files, $n_pairs, scalar @nodes;
} else {
    print $json;
}
