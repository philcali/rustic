#!/bin/bash

# Generate registry index from built binaries
set -e

REGISTRY_DIR="registry"
BINARIES_DIR="binaries"

# Get version from environment or Cargo.toml
if [ -n "$SOFTWARE_VERSION" ]; then
    VERSION="$SOFTWARE_VERSION"
elif [ -n "$GITHUB_REF_NAME" ]; then
    VERSION="$GITHUB_REF_NAME"
else
    VERSION=$(grep '^version = ' pandemic-daemon/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/')
fi

echo "Using version: $VERSION"

BASE_URL="https://philcali.github.io/rustic"

# Supported architectures
ARCHS=("x86_64-unknown-linux-gnu" "armv7-unknown-linux-gnueabihf" "aarch64-unknown-linux-gnu")
ARCH_NAMES=("x86_64" "armv7" "aarch64")

mkdir -p "$REGISTRY_DIR" "$BINARIES_DIR"

# Discover all relevant binaries from first architecture
INFECTIONS=()
CORE_COMPONENTS=()
cd target/${ARCHS[0]}/release

# Find infection binaries
for binary in *-infection pandemic-*; do
    echo "Checking binary: $binary"
    if [ "$binary" = "pandemic-agent" ] || [ "$binary" = "pandemic" ]; then
        continue
    fi
    if [ -f "$binary" ] && [ -x "$binary" ]; then
        INFECTIONS+=("$binary")
        echo "Found infection: $binary"
    fi
done

# Find core components
for binary in pandemic pandemic-agent; do
    echo "Checking binary: $binary"
    if [ -f "$binary" ] && [ -x "$binary" ]; then
        CORE_COMPONENTS+=("$binary")
        echo "Found core component: $binary"
    fi
done
cd ../../..

# Generate index.json
cat > "$REGISTRY_DIR/index.json" << EOF
{
  "name": "Pandemic Infections Registry",
  "description": "Official registry for pandemic infections and core components",
  "infections": {
EOF

# Add infections to index
first_item=true
for infection in "${INFECTIONS[@]}"; do
    name=$(basename "$infection")
    
    if [ "$first_item" = false ]; then
        echo "," >> "$REGISTRY_DIR/index.json"
    fi
    first_item=false
    
    cat >> "$REGISTRY_DIR/index.json" << EOF
    "$name": {
      "name": "$name",
      "latest_version": "$VERSION",
      "description": "Pandemic infection: $name",
      "type": "infection",
      "manifest_url": "$BASE_URL/registry/$name.json"
    }
EOF
done

# Add core components to index
for component in "${CORE_COMPONENTS[@]}"; do
    name=$(basename "$component")
    
    if [ "$first_item" = false ]; then
        echo "," >> "$REGISTRY_DIR/index.json"
    fi
    first_item=false
    
    description="Core pandemic component: $name"
    if [ "$name" = "pandemic" ]; then
        description="Pandemic daemon - core hub for managing infections"
    elif [ "$name" = "pandemic-agent" ]; then
        description="Pandemic agent - privileged operations handler"
    fi
    
    cat >> "$REGISTRY_DIR/index.json" << EOF
    "$name": {
      "name": "$name",
      "latest_version": "$VERSION",
      "description": "$description",
      "type": "core",
      "manifest_url": "$BASE_URL/registry/$name.json"
    }
EOF
done

cat >> "$REGISTRY_DIR/index.json" << EOF
  }
}
EOF

# Generate manifests for infections
for infection in "${INFECTIONS[@]}"; do
    name=$(basename "$infection")
    
    # Build platforms array for all architectures
    platforms="["
    for i in "${!ARCHS[@]}"; do
        arch_target="${ARCHS[$i]}"
        arch_name="${ARCH_NAMES[$i]}"
        
        if [ -f "target/$arch_target/release/$infection" ]; then
            checksum=$(sha256sum "target/$arch_target/release/$infection" | cut -d' ' -f1)
            
            if [ $i -gt 0 ]; then
                platforms="$platforms,"
            fi
            
            platforms="$platforms
    {
      \"os\": \"linux\",
      \"arch\": \"$arch_name\",
      \"binary_url\": \"$BASE_URL/binaries/$arch_name/$name\",
      \"checksum\": \"$checksum\"
    }"
            
            # Copy binary to arch-specific directory
            mkdir -p "$BINARIES_DIR/$arch_name"
            cp "target/$arch_target/release/$infection" "$BINARIES_DIR/$arch_name/"
        fi
    done
    platforms="$platforms
  ]"
    
    cat > "$REGISTRY_DIR/$name.json" << EOF
{
  "name": "$name",
  "version": "$VERSION",
  "description": "Pandemic infection that integrates with the pandemic daemon system",
  "author": "Pandemic Team",
  "homepage": "https://github.com/philcali/rustic",
  "license": "MIT",
  "keywords": ["infection", "pandemic", "daemon"],
  "dependencies": [],
  "platforms": $platforms
}
EOF

    echo "Generated manifest for $name"
done

# Generate manifests for core components
for component in "${CORE_COMPONENTS[@]}"; do
    name=$(basename "$component")
    
    description="Core pandemic component"
    keywords='["pandemic", "daemon", "core"]'
    if [ "$name" = "pandemic" ]; then
        description="Pandemic daemon - core hub managing plugin registry, IPC, and health monitoring"
    elif [ "$name" = "pandemic-agent" ]; then
        description="Pandemic agent - privileged operations handler for system management"
        keywords='["pandemic", "agent", "privileged", "system"]'
    fi
    
    # Build platforms array for all architectures
    platforms="["
    for i in "${!ARCHS[@]}"; do
        arch_target="${ARCHS[$i]}"
        arch_name="${ARCH_NAMES[$i]}"
        
        if [ -f "target/$arch_target/release/$component" ]; then
            checksum=$(sha256sum "target/$arch_target/release/$component" | cut -d' ' -f1)
            
            if [ $i -gt 0 ]; then
                platforms="$platforms,"
            fi
            
            platforms="$platforms
    {
      \"os\": \"linux\",
      \"arch\": \"$arch_name\",
      \"binary_url\": \"$BASE_URL/binaries/$arch_name/$name\",
      \"checksum\": \"$checksum\"
    }"
            
            # Copy binary to arch-specific directory
            mkdir -p "$BINARIES_DIR/$arch_name"
            cp "target/$arch_target/release/$component" "$BINARIES_DIR/$arch_name/"
        fi
    done
    platforms="$platforms
  ]"
    
    cat > "$REGISTRY_DIR/$name.json" << EOF
{
  "name": "$name",
  "version": "$VERSION",
  "description": "$description",
  "author": "Pandemic Team",
  "homepage": "$BASE_URL",
  "license": "MIT",
  "keywords": $keywords,
  "dependencies": [],
  "platforms": $platforms
}
EOF

    echo "Generated manifest for $name"
done

total_items=$((${#INFECTIONS[@]} + ${#CORE_COMPONENTS[@]}))
echo "Registry index generated with $total_items items (${#INFECTIONS[@]} infections, ${#CORE_COMPONENTS[@]} core components)"