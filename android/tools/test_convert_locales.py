import sys
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import convert_locales as c  # noqa: E402


class ConvertTextTest(unittest.TestCase):
    def test_placeholder_order_is_alphabetical(self):
        self.assertEqual(c.convert_text("{{max}} of {{available}}"), "%2$s of %1$s")

    def test_repeated_placeholder_shares_index(self):
        self.assertEqual(c.convert_text("{{a}} {{a}}"), "%1$s %1$s")

    def test_escapes(self):
        self.assertEqual(c.convert_text("Don't \"x\" & <b>"), "Don\\'t \\\"x\\\" &amp; &lt;b&gt;")

    def test_percent_only_escaped_when_formatted(self):
        self.assertEqual(c.convert_text("100%"), "100%")
        self.assertEqual(c.convert_text("{{n}}%"), "%1$s%%")

    def test_resource_name(self):
        self.assertEqual(c.resource_name("app.sidestore_stable"), "app_sidestore_stable")
        self.assertEqual(c.resource_name("a.b-c"), "a_b_c")


class ConvertRealLocalesTest(unittest.TestCase):
    def test_outputs_are_valid_xml_with_matching_names(self):
        outputs, key_map = c.convert()
        self.assertEqual(set(outputs), {"values", "values-zh-rCN"})
        names = {}
        for values_dir, xml in outputs.items():
            root = ET.fromstring(xml)
            names[values_dir] = {el.get("name") for el in root}
        self.assertEqual(names["values"], set(key_map.values()))
        self.assertTrue(names["values-zh-rCN"] <= names["values"])

    def test_arrays_are_string_arrays(self):
        outputs, _ = c.convert()
        self.assertIn('<string-array name="error_suggestions_auth">', outputs["values"])


if __name__ == "__main__":
    unittest.main()
