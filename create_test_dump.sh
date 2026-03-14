#!/bin/bash
# Create a test NAND dump file for testing the viewer

set -e

FILENAME="${1:-test_dump.bin}"
SIZE="${2:-1M}"

echo "Creating test NAND dump file: $FILENAME"
echo "Size: $SIZE"

# Parse size to determine if it's large enough to use sparse file
# Convert to bytes for comparison
SIZE_BYTES=0
if [[ $SIZE =~ ^([0-9]+)([KMGT]?)$ ]]; then
    NUM="${BASH_REMATCH[1]}"
    UNIT="${BASH_REMATCH[2]}"
    case $UNIT in
        K) SIZE_BYTES=$((NUM * 1024)) ;;
        M) SIZE_BYTES=$((NUM * 1024 * 1024)) ;;
        G) SIZE_BYTES=$((NUM * 1024 * 1024 * 1024)) ;;
        T) SIZE_BYTES=$((NUM * 1024 * 1024 * 1024 * 1024)) ;;
        *) SIZE_BYTES=$NUM ;;
    esac
fi

# If size is > 100 MB, use sparse file approach
if [ $SIZE_BYTES -gt $((100 * 1024 * 1024)) ]; then
    echo "Creating sparse file (large size detected)..."
    echo "Writing 10 MB of random data..."
    dd if=/dev/urandom of="$FILENAME" bs=1M count=10 2>/dev/null
    
    echo "Extending to $SIZE (sparse)..."
    truncate -s "$SIZE" "$FILENAME"
    
    ACTUAL_SIZE=$(du -h "$FILENAME" | cut -f1)
    APPARENT_SIZE=$(ls -lh "$FILENAME" | awk '{print $5}')
    
    echo ""
    echo "✓ Test dump file created successfully!"
    echo "  File: $FILENAME"
    echo "  Apparent size: $APPARENT_SIZE"
    echo "  Actual disk usage: $ACTUAL_SIZE (sparse file)"
else
    echo "Creating regular file with random data..."
    dd if=/dev/urandom of="$FILENAME" bs=1M count=$NUM 2>/dev/null
    
    ACTUAL_SIZE=$(du -h "$FILENAME" | cut -f1)
    
    echo ""
    echo "✓ Test dump file created successfully!"
    echo "  File: $FILENAME"
    echo "  Size: $ACTUAL_SIZE"
fi

echo ""
echo "To use with NAND Flash Viewer:"
echo "  cargo run --release -- $FILENAME"
echo ""
echo "When prompted, use these parameters:"
echo "  Page length: 2048"
echo "  Block size: 128"
