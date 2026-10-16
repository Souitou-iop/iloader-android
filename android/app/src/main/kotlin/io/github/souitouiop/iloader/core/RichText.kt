package io.github.souitouiop.iloader.core

/** Splits `text <tag>inner</tag> text` markers used by a few locale strings (`<discord>`, `<translation>`, ...). */
object RichText {
    data class Part(val text: String, val tag: String? = null)

    private val tagged = Regex("<([a-z_]+)>(.*?)</\\1>", RegexOption.DOT_MATCHES_ALL)

    fun split(text: String): List<Part> {
        val out = mutableListOf<Part>()
        var last = 0
        for (m in tagged.findAll(text)) {
            if (m.range.first > last) out += Part(text.substring(last, m.range.first))
            out += Part(m.groupValues[2], m.groupValues[1])
            last = m.range.last + 1
        }
        if (last < text.length) out += Part(text.substring(last))
        return out
    }
}
