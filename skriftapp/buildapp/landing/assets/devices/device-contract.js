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
      "model": "MacBook Pro 14-inch (M5, 2025)",
      "bounds_size_m": [
        0.3128499686717987,
        0.21444788575172424,
        0.2790847159922123
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
          0.0,
          0.1126074,
          -0.026269
        ],
        "corners_m": [
          [
            -0.1512,
            0.0177535,
            -0.000853
          ],
          [
            0.1512,
            0.0177535,
            -0.000853
          ],
          [
            0.1512,
            0.2074614,
            -0.051685
          ],
          [
            -0.1512,
            0.2074614,
            -0.051685
          ]
        ],
        "corner_radius_m": 0.0045,
        "surface_clearance_m": 0.000105
      },
      "closed_height_m": 0.0155,
      "lid_size_m": [
        0.3126,
        0.211,
        0.0041
      ],
      "lid_corner_radius_m": 0.006,
      "lid_open_angle_deg": 105.0,
      "lid_hinge_node": "laptop_lid",
      "lid_hinge_axis": "+X",
      "lid_hinge_closed_rotation_deg": 90.0,
      "lid_hinge_default_rotation_deg": -15.0,
      "hinge_pivot_m": [
        0.0,
        0.01175,
        0.0004
      ]
    },
    "tablet": {
      "model": "iPad Pro 11-inch (M5, 2025)",
      "bounds_size_m": [
        0.2502500042319298,
        0.17805000394582748,
        0.007850000634789467
      ],
      "body_size_m": [
        0.2497,
        0.1775,
        0.0053
      ],
      "body_corner_radius_m": 0.01505,
      "screen": {
        "node": "screen",
        "size_m": [
          0.2328333333333333,
          0.16048181818181817
        ],
        "center_m": [
          0,
          0,
          0.00285
        ],
        "corners_m": [
          [
            -0.11641666666666665,
            -0.08024090909090908,
            0.00285
          ],
          [
            0.11641666666666665,
            -0.08024090909090908,
            0.00285
          ],
          [
            0.11641666666666665,
            0.08024090909090908,
            0.00285
          ],
          [
            -0.11641666666666665,
            0.08024090909090908,
            0.00285
          ]
        ],
        "corner_radius_m": 0.0066,
        "surface_clearance_m": 9e-05
      }
    },
    "phone": {
      "model": "iPhone 17 Pro Max (2025)",
      "bounds_size_m": [
        0.07890000566840172,
        0.16374999284744263,
        0.014270000159740448
      ],
      "body_size_m": [
        0.078,
        0.1634,
        0.00875
      ],
      "body_corner_radius_m": 0.0154,
      "screen": {
        "node": "screen",
        "size_m": [
          0.07288695652173913,
          0.15836347826086955
        ],
        "center_m": [
          0,
          0,
          0.00459
        ],
        "corners_m": [
          [
            -0.03644347826086956,
            -0.07918173913043478,
            0.00459
          ],
          [
            0.03644347826086956,
            -0.07918173913043478,
            0.00459
          ],
          [
            0.03644347826086956,
            0.07918173913043478,
            0.00459
          ],
          [
            -0.03644347826086956,
            0.07918173913043478,
            0.00459
          ]
        ],
        "corner_radius_m": 0.014,
        "surface_clearance_m": 0.0001
      }
    }
  }
});
