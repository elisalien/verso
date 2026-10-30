/*{
    "DESCRIPTION": "Example ISF filter: darkened horizontal lines.",
    "CATEGORIES": ["Stylize"],
    "INPUTS": [
        {"NAME": "inputImage", "TYPE": "image"},
        {"NAME": "lines", "TYPE": "float", "MIN": 50.0, "MAX": 1000.0, "DEFAULT": 300.0},
        {"NAME": "darkness", "TYPE": "float", "MIN": 0.0, "MAX": 1.0, "DEFAULT": 0.5}
    ]
}*/

void main() {
    vec4 color = IMG_THIS_PIXEL(inputImage);
    float line = step(0.5, fract(isf_FragNormCoord.y * lines));
    gl_FragColor = vec4(color.rgb * mix(1.0, 1.0 - darkness, line), color.a);
}
