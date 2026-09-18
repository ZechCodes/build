const deepFreeze = value => {
  Object.values(value).forEach(child => {
    if (child && typeof child === 'object') deepFreeze(child);
  });
  return Object.freeze(value);
};

export const DEVICE_CONTRACT = deepFreeze({
  "version": 1,
  "units": "meters",
  "axes": {
    "up": "+Y",
    "forward": "+Z",
    "right": "+X"
  },
  "origin": "device center; laptop origin is rear-center hinge projection on base underside",
  "devices": {
    "laptop": {
      "bounds_size_m": [
        0.3126,
        0.2215,
        0.2222
      ],
      "body_size_m": [
        0.3126,
        0.011,
        0.2212
      ],
      "body_corner_radius_m": 0.0095,
      "screen": {
        "node": "screen",
        "size_m": [
          0.3024,
          0.1964
        ],
        "center_m": [
          0,
          0.116,
          0.00112
        ],
        "corners_m": [
          [
            -0.1512,
            0.0178,
            0.00112
          ],
          [
            0.1512,
            0.0178,
            0.00112
          ],
          [
            0.1512,
            0.2142,
            0.00112
          ],
          [
            -0.1512,
            0.2142,
            0.00112
          ]
        ],
        "corner_radius_m": 0.0045
      },
      "lid_size_m": [
        0.3126,
        0.211,
        0.004
      ],
      "lid_corner_radius_m": 0.006,
      "hinge_pivot_m": [
        0,
        0.0096,
        0.0004
      ]
    },
    "tablet": {
      "bounds_size_m": [
        0.2816,
        0.2155,
        0.0051
      ],
      "body_size_m": [
        0.2816,
        0.2155,
        0.0051
      ],
      "body_corner_radius_m": 0.0095,
      "screen": {
        "node": "screen",
        "size_m": [
          0.264,
          0.198
        ],
        "center_m": [
          0,
          0,
          0.00267
        ],
        "corners_m": [
          [
            -0.132,
            -0.099,
            0.00267
          ],
          [
            0.132,
            -0.099,
            0.00267
          ],
          [
            0.132,
            0.099,
            0.00267
          ],
          [
            -0.132,
            0.099,
            0.00267
          ]
        ],
        "corner_radius_m": 0.0068
      }
    },
    "phone": {
      "bounds_size_m": [
        0.0719,
        0.15,
        0.00875
      ],
      "body_size_m": [
        0.0719,
        0.15,
        0.00875
      ],
      "body_corner_radius_m": 0.014,
      "screen": {
        "node": "screen",
        "size_m": [
          0.0664,
          0.14435
        ],
        "center_m": [
          0,
          0,
          0.0045
        ],
        "corners_m": [
          [
            -0.0332,
            -0.072175,
            0.0045
          ],
          [
            0.0332,
            -0.072175,
            0.0045
          ],
          [
            0.0332,
            0.072175,
            0.0045
          ],
          [
            -0.0332,
            0.072175,
            0.0045
          ]
        ],
        "corner_radius_m": 0.0115
      }
    }
  }
});
