{
  "targets": [
    {
      "target_name": "linux_dnd",
      "sources": ["src/linux_dnd.cc"],
      "cflags!": ["-fno-exceptions"],
      "cflags_cc!": ["-fno-exceptions"],
      "conditions": [
        ["OS=='linux'", {
          "cflags": ["<!@(pkg-config --cflags gtk4)"],
          "libraries": ["<!@(pkg-config --libs gtk4)"],
          "ldflags": ["<!@(pkg-config --libs-only-L gtk4)"]
        }]
      ],
      "include_dirs": [
        "<!@(node -p \"require('node-addon-api').include\")"
      ]
    }
  ]
}
